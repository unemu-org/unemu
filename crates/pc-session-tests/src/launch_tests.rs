// SPDX-License-Identifier: AGPL-3.0-or-later
//! Equality of the launch factory and the verified-file assembler, including
//! the version-1 journal bytes (core-contract.md §14.3).

use machine_file::FileMachineExt;
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

fn compare_outputs(created: &mut dyn Machine, direct: &mut dyn Machine) {
    created.set_outputs(Outputs::NONE.with_audio(true));
    direct.set_outputs(Outputs::NONE.with_audio(true));
    for steps in [100, 200, 8000] {
        created.input(InputEvent::ReleaseAll).unwrap();
        direct.input(InputEvent::ReleaseAll).unwrap();
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
}

fn compare_session(created: &mut PcMachine, direct: &mut PcMachine, session: u8) -> Vec<u8> {
    assert_eq!(cmos_byte(direct), session - 1);
    compare_outputs(created, direct);
    assert_eq!(cmos_byte(direct), session);
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
    for machine in [&mut *created, &mut *direct] {
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
    written
}

#[test]
fn launch_and_verified_file_sessions_have_identical_output_and_journal_bytes() {
    let (bytes, source) = fixture();
    let options = assembly::AssemblyOptions {
        allow_unlisted_firmware: true,
    };
    let lowered = launch::lower(&launch::decode(&bytes).unwrap(), &source, options).unwrap();
    let storage = Rc::new(MemoryStorage::new());
    let temp = machine_file::test_support::TempDir::new("launch-comparison");
    let mut direct_config = lowered.machine.clone();
    let mut bases = BTreeMap::new();
    for (key, (sha256, _)) in &lowered.bases {
        let path = temp.path().join(key.file_name().unwrap());
        let data = &source.blobs[sha256];
        std::fs::write(&path, data).unwrap();
        bases.insert(
            path.clone(),
            journal_files::VerifiedBase::from_bytes(data).unwrap(),
        );
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
            machine_file::assemble_with_verified_bases(&direct_config, &policy(), options, &bases)
                .unwrap();
        let mut direct = PcMachine::from_assembled(&direct_config, direct).unwrap();
        let mut created = Box::new(
            PcCore
                .create_machine(&request(&bytes, &source, storage.clone()))
                .unwrap(),
        );
        let written = compare_session(&mut created, &mut direct, session);
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
            let scan = journal_files::scan(&journal, &base, base.len() as u64);
            assert_eq!(scan.report.status, journal_files::Status::Valid);
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
    let temp = machine_file::test_support::TempDir::new("launch-append");
    let path = temp.path().join("extra.img");
    let data = vec![0xe5; 368640];
    std::fs::write(&path, &data).unwrap();
    let base = journal_files::VerifiedBase::open(&path).unwrap();
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

fn stored_fixture() -> (
    machine_file::test_support::TempDir,
    image_store::Store,
    image_store::Descriptor,
) {
    stored_fixture_with_zero(false)
}

fn stored_fixture_with_zero(
    zero_disk: bool,
) -> (
    machine_file::test_support::TempDir,
    image_store::Store,
    image_store::Descriptor,
) {
    use image_store::{media_type as mt, Descriptor, Manifest, Store};
    let (bytes, source) = fixture();
    let mut document = launch::decode(&bytes).unwrap();
    let temp = machine_file::test_support::TempDir::new("stored-launch");
    let store = Store::open(temp.path().join("store")).unwrap();
    let mut layers = Vec::new();
    for (digest, kind) in [
        (document.config.firmware[0].blob, mt::FIRMWARE),
        (document.config.media[0].head, mt::FLOPPY),
        (document.config.nvram.as_ref().unwrap().head, pc_image::CMOS),
    ] {
        let blob = store
            .import(source.blobs[&digest.sha256()].as_slice(), None)
            .unwrap();
        layers.push(Descriptor::new(kind, blob.digest, blob.size));
    }
    let metadata = store
        .import(
            br#"{"schema":1,"platform":"ibm-pc","labels":[],"titles":[]}"#.as_slice(),
            None,
        )
        .unwrap();
    layers.push(Descriptor::new(
        mt::METADATA,
        metadata.digest,
        metadata.size,
    ));
    if zero_disk {
        let zero = pc_image::zero::ZeroBase::new(1, 1, 32).unwrap();
        let bytes = zero.encode().unwrap();
        let blob = store.import(bytes.as_slice(), None).unwrap();
        let digest: Digest = blob.digest.to_string().parse().unwrap();
        layers.insert(
            2,
            Descriptor::new(pc_image::DISK_ZERO, blob.digest, blob.size),
        );
        document.config.media.push(pc_config::image::Medium {
            id: "c".into(),
            source: "assets/c.img".into(),
            head: digest,
            name: None,
            policy: pc_config::image::Policy::Journal,
            geometry: pc_config::image::Geometry {
                cylinders: 1,
                heads: 1,
                sectors: 32,
                sector_bytes: 512,
            },
        });
        document.config.slots.push(pc_config::image::Slot {
            controller: "ide".into(),
            unit: 0,
            catalogue: vec!["c".into()],
            catalogue_mode: false,
            inserted: Some("c".into()),
        });
        let card: toml::Table =
            "kind='ide'\n[[drives]]\nchannel='master'\ngeometry={cylinders=1,heads=1,sectors=32}\n"
                .parse()
                .unwrap();
        document.config.machine["card"]
            .as_array_mut()
            .unwrap()
            .push(toml::Value::Table(card));
        document.config.machine["firmware"]["bios"]["provides"]
            .as_array_mut()
            .unwrap()
            .push("fixed-disk".into());
    }
    let config = document.config.encode().unwrap();
    let config = store.import(config.as_bytes(), None).unwrap();
    let manifest = Manifest::new_with_types(
        launch::PLATFORM,
        Descriptor::new(mt::CONFIG, config.digest, config.size),
        layers,
        pc_image::PAYLOAD_TYPES,
    )
    .unwrap();
    let image = store
        .import_manifest_with_types(&manifest, pc_image::PAYLOAD_TYPES)
        .unwrap();
    store
        .update_tag("test:launch", &image, None, &pc_image::PcProvider)
        .unwrap();
    (temp, store, image)
}

fn contract_options(prepared: core_launch::Prepared) -> core_launch::Prepared {
    prepared.with_options(vec![
        ("allow-unlisted-firmware".into(), "true".into()),
        ("speaker-trace".into(), "true".into()),
    ])
}

#[test]
fn stored_launch_and_existing_start_match_across_restarts_in_both_directions() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, image) = stored_fixture();
        let owners = [
            store.workspace("files").unwrap(),
            store.workspace("objects").unwrap(),
        ];
        for session in 1..=5 {
            // Alternate paths in each workspace: every journal crosses the seam
            // in both directions, including retained histories.
            let (old, new) = if session % 2 == 1 {
                (&owners[0], &owners[1])
            } else {
                (&owners[1], &owners[0])
            };
            let mut prepared =
                pc_image::workspace::prepare_start(old, "test:launch", false).unwrap();
            prepared
                .edit_launch(|doc| {
                    for row in &mut doc.records {
                        row.keep_generations = 3;
                    }
                })
                .unwrap();
            let assembled = machine_file::assemble_with_verified_bases(
                &prepared.machine,
                &policy(),
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
                &prepared.bases,
            )
            .unwrap();
            let mut direct = PcMachine::from_assembled(&prepared.machine, assembled).unwrap();
            let mut prepared =
                pc_image::workspace::prepare_start(new, "test:launch", false).unwrap();
            let doc = prepared.launch.as_ref().unwrap().document().clone();
            prepared
                .edit_launch(|doc| {
                    for row in &mut doc.records {
                        row.keep_generations = 3;
                    }
                })
                .unwrap();
            let prepared =
                pc_image::launch::from_session(prepared, &PcCore, "Stored PC".into()).unwrap();
            assert_eq!(prepared.image, image.digest);
            let mut created = core_launch::start_with(&contract_options(prepared.prepared), |r| {
                PcCore.create_machine(r)
            })
            .unwrap();
            compare_session(&mut created, &mut direct, session);
            drop(created);
            drop(direct);
            for row in doc.records {
                let left = std::fs::read(old.path().join("media").join(&row.name)).unwrap();
                let right = std::fs::read(new.path().join("media").join(&row.name)).unwrap();
                assert_eq!(left, right, "{}, session {session}", row.name);
                let base = if row.medium == "a" {
                    vec![0xf6; 368640]
                } else {
                    vec![0; 64]
                };
                let scan = journal_files::scan(&left, &base, base.len() as u64);
                assert_eq!(scan.report.status, journal_files::Status::Valid);
                assert_eq!(scan.report.generations.len(), usize::from(session.min(3)));
                assert_eq!(
                    scan.report.header.unwrap().anchor_generation,
                    if session > 3 {
                        u64::from(session - 2)
                    } else {
                        0
                    }
                );
            }
        }
    });
}

#[test]
fn boxed_stored_launch_matches_existing_start_and_its_journals() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, _) = stored_fixture();
        let old = store.workspace("old-boxed").unwrap();
        let new = store.workspace("boxed").unwrap();
        let prepared =
            pc_image::launch::prepare_workspace(&new, "test:launch", true, &PcCore, "Boxed".into())
                .unwrap();
        let prepared = contract_options(prepared.prepared);
        for session in 1..=2 {
            let old_session =
                pc_image::workspace::prepare_start(&old, "test:launch", false).unwrap();
            let assembled = machine_file::assemble_with_verified_bases(
                &old_session.machine,
                &policy(),
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
                &old_session.bases,
            )
            .unwrap();
            let mut direct = PcMachine::from_assembled(&old_session.machine, assembled).unwrap();
            let mut boxed = core_launch::start(&prepared).unwrap();
            compare_outputs(boxed.as_mut(), &mut direct);
            assert_eq!(boxed.finish().unwrap(), direct.finish().unwrap());
            drop(boxed);
            drop(direct);
            for name in ["a.upfj", "cmos.upfj"] {
                let bytes = std::fs::read(old.path().join("media").join(name)).unwrap();
                assert_eq!(
                    bytes,
                    std::fs::read(new.path().join("media").join(name)).unwrap()
                );
                if name == "a.upfj" {
                    let scan = journal_files::scan(&bytes, &vec![0xf6; 368640], 368640);
                    let image = scan.image.unwrap();
                    assert_eq!(image[..512], vec![session; 512]);
                    assert_eq!(image[512..1024], vec![session ^ 0x50; 512]);
                }
            }
        }
    });
}

#[test]
fn stored_launch_locks_exclude_another_start_and_the_rollback_tool_and_release_on_drop() {
    use journal_files::offline::{Access, JournalTool};
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, _) = stored_fixture();
        let owner = store.workspace("locks").unwrap();
        let session = pc_image::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let drive = &session.machine.drives[0];
        let assembly::ImageSource::File(base) = &drive.media.as_ref().unwrap().entries[0].image
        else {
            panic!("expected file")
        };
        let base = base.clone();
        let journal = owner.path().join("media/a.upfj");
        let prepared = pc_image::launch::from_session(session, &PcCore, "Locks".into()).unwrap();
        let prepared = contract_options(prepared.prepared);
        let mut first = core_launch::start(&prepared).unwrap();
        let second = core_launch::start(&prepared).err().unwrap();
        assert_eq!(second.kind, ErrorKind::Usage);
        assert_eq!(
            second.message,
            "lock: journal is locked by another session or tool"
        );
        let tool_error = JournalTool::open(&base, &journal, Access::Mutate)
            .err()
            .unwrap();
        assert!(tool_error
            .to_string()
            .contains("journal is locked by another session or tool"));
        let old = pc_image::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let old_error = machine_file::assemble_with_verified_bases(
            &old.machine,
            &policy(),
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &old.bases,
        )
        .err()
        .unwrap();
        assert!(old_error.to_string().contains(&second.message));
        first.run(RunLimit::steps(8300)).unwrap();
        first.finish().unwrap();
        drop(first);
        let mut tool = JournalTool::open(&base, &journal, Access::Mutate).unwrap();
        assert_eq!(core_launch::start(&prepared).err().unwrap(), second);
        tool.rollback(0, &owner.path().join("rollback.backup"))
            .unwrap();
        drop(tool);
        let mut restarted = core_launch::start(&prepared).unwrap();
        restarted.run(RunLimit::steps(8300)).unwrap();
        restarted.finish().unwrap();
    });
}

#[test]
fn stored_content_is_exact_and_changed_verified_files_fail_as_content() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, _) = stored_fixture();
        let owner = store.workspace("content").unwrap();
        let session = pc_image::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let doc = session.launch.as_ref().unwrap().document().clone();
        let assembly::ImageSource::File(path) =
            &session.machine.drives[0].media.as_ref().unwrap().entries[0].image
        else {
            panic!("file")
        };
        let path = path.clone();
        let prepared = pc_image::launch::from_session(session, &PcCore, "Content".into()).unwrap();
        let prepared = contract_options(prepared.prepared);
        core_launch::start_with(&prepared, |request| {
            for row in &doc.content {
                let content = request.content.open(&row.sha256, row.size)?;
                let mut bytes = vec![0; row.size as usize];
                match content {
                    Content::Bytes(data) => bytes = data,
                    Content::Reader(reader) => {
                        reader.read_at(0, &mut bytes)?;
                        let mut unaligned = [0; 777];
                        reader.read_at(511, &mut unaligned)?;
                        assert_eq!(unaligned, bytes[511..1288]);
                        assert_eq!(
                            reader.read_at(row.size, &mut [0]).unwrap_err().kind,
                            ErrorKind::Usage
                        );
                    }
                    _ => panic!("unsupported content"),
                }
                assert_eq!(Digest::of(&bytes).sha256(), row.sha256);
                assert_eq!(bytes.len() as u64, row.size);
                assert_eq!(
                    request
                        .content
                        .open(&row.sha256, row.size + 1)
                        .err()
                        .unwrap()
                        .kind,
                    ErrorKind::Content
                );
            }
            assert_eq!(
                request.content.open(&[0x99; 32], 64).err().unwrap().kind,
                ErrorKind::Content
            );
            assert_eq!(
                request.storage.open("unlisted.upfj").err().unwrap().kind,
                ErrorKind::Usage
            );
            assert!(!owner.path().join("media/a.upfj").exists());
            Ok(())
        })
        .unwrap();
        let mut machine = core_launch::start(&prepared).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(path, bytes).unwrap();
        assert_eq!(
            core_launch::start(&prepared).err().unwrap().kind,
            ErrorKind::Content
        );
        assert_eq!(
            machine.run(RunLimit::steps(8300)).unwrap_err().kind,
            ErrorKind::Content
        );
    });
}

#[test]
fn stored_compressed_floppy_and_zero_disk_plans_restart_after_thread_transfer() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, _) = stored_fixture_with_zero(true);
        pc_image::compress_image(&store, "test:launch", "test:compressed").unwrap();
        let owner = store.workspace("sparse").unwrap();
        let session = pc_image::workspace::prepare_start(&owner, "test:compressed", false).unwrap();
        assert_eq!(session.zero.len(), 1);
        assert!(
            session
                .bases
                .values()
                .filter(|b| b.metadata().is_none())
                .count()
                >= 2
        );
        let doc = session.launch.as_ref().unwrap().document().clone();
        let zero = doc
            .content
            .iter()
            .find(|c| c.digest == doc.config.media[1].head)
            .unwrap();
        let identity = (zero.sha256, zero.size);
        let prepared = pc_image::launch::from_session(session, &PcCore, "Sparse".into()).unwrap();
        let prepared = contract_options(prepared.prepared);
        std::thread::spawn(move || {
            for _ in 0..2 {
                core_launch::start_with(&prepared, |request| {
                    let Content::Reader(reader) = request.content.open(&identity.0, identity.1)?
                    else {
                        panic!("sparse reader")
                    };
                    let mut bytes = [1; 777];
                    reader.read_at(511, &mut bytes)?;
                    assert_eq!(bytes, [0; 777]);
                    let mut machine = PcCore.create_machine(request)?;
                    machine.run(RunLimit::steps(8300))?;
                    machine.finish()?;
                    Ok(())
                })
                .unwrap();
            }
        })
        .join()
        .unwrap();
        assert!(!store
            .root()
            .join("cache/unmaterialized")
            .join(doc.config.media[1].head.hex())
            .exists());
        assert!(owner.path().join("media/c.upfj").is_file());
    });
}

#[test]
fn stored_prepare_helpers_keep_pins_machine_settings_and_copy_journals() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, image) = stored_fixture();
        let owner = store.workspace("workspace").unwrap();
        let first = pc_image::launch::prepare_workspace(
            &owner,
            "test:launch",
            true,
            &PcCore,
            "Workspace".into(),
        )
        .unwrap();
        assert_eq!(first.image, image.digest);
        let mut machine = core_launch::start(&contract_options(first.prepared)).unwrap();
        machine.run(RunLimit::steps(8300)).unwrap();
        machine.finish().unwrap();
        drop(machine);
        let pinned =
            pc_image::launch::prepare_pinned(&owner, &image, false, &PcCore, "Pinned".into())
                .unwrap();
        assert_eq!(pinned.image, image.digest);
        let id = pc_image::machine::create(&store, &image, "test:launch", Some("Machine"), None)
            .unwrap();
        let owner = pc_image::machine::own(&store, &id).unwrap();
        let old = pc_image::machine::prepare(&owner).unwrap();
        let old_memory = old.session.machine.memory.clone();
        let mut hardware = old
            .session
            .launch
            .unwrap()
            .document()
            .config
            .machine
            .clone();
        hardware.insert(
            "memory".into(),
            toml::Value::Table("conventional_kib=640\nextended_kib=2048".parse().unwrap()),
        );
        store
            .edit_machines(|machines| {
                machines.get_mut(&id).unwrap().settings = Some(hardware);
                Ok(())
            })
            .unwrap();
        let expected_memory = pc_image::machine::prepare(&owner)
            .unwrap()
            .session
            .machine
            .memory;
        assert_ne!(expected_memory, old_memory);
        let prepared =
            pc_image::launch::prepare_machine(&owner, &PcCore, "Machine".into()).unwrap();
        assert_eq!(prepared.image, image.digest);
        assert!(prepared.inserted.is_empty());
        let mut machine = core_launch::start_with(&contract_options(prepared.prepared), |r| {
            PcCore.create_machine(r)
        })
        .unwrap();
        assert_eq!(machine.config().memory, expected_memory);
        machine.run(RunLimit::steps(8300)).unwrap();
        machine.finish().unwrap();
        drop(machine);
        let before = std::fs::read(owner.path().join("media/a.upfj")).unwrap();
        let copy = store.workspace("copy").unwrap();
        let prepared =
            pc_image::launch::prepare_copy(&owner, &copy, &PcCore, "Copy".into()).unwrap();
        assert_eq!(prepared.image, image.digest);
        assert_eq!(
            std::fs::read(copy.path().join("media/a.upfj")).unwrap(),
            before
        );
        let mut machine = core_launch::start_with(&contract_options(prepared.prepared), |r| {
            PcCore.create_machine(r)
        })
        .unwrap();
        assert_eq!(machine.config().memory, expected_memory);
        machine.run(RunLimit::steps(8300)).unwrap();
        machine.finish().unwrap();
        drop(machine);
        let prepared = pc_image::launch::resume_copy(&copy, &PcCore, "Copy again".into()).unwrap();
        core_launch::start(&contract_options(prepared.prepared))
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(
            std::fs::read(owner.path().join("media/a.upfj")).unwrap(),
            before
        );
    });
}

#[test]
fn edited_catalogue_records_keep_their_external_journal_paths() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (temp, store, _) = stored_fixture();
        let owner = store.workspace("catalogue").unwrap();
        let mut session = pc_image::workspace::prepare_start(&owner, "test:launch", false).unwrap();
        let base_path = temp.path().join("external.img");
        let journal = temp.path().join("external.upfj");
        std::fs::write(&base_path, vec![0xf6; 368640]).unwrap();
        let base = journal_files::VerifiedBase::open(&base_path).unwrap();
        session
            .replace_floppy_catalogue(
                0,
                vec![assembly::FloppyMedium {
                    image: assembly::ImageSource::File(base_path.clone()),
                    expected_sha256: Some(base.identity().base_digest),
                    write: assembly::WriteMode::journal(journal.clone()),
                    name: "External".into(),
                }],
                Some(0),
                [(base_path.clone(), base)].into(),
            )
            .unwrap();
        let prepared =
            pc_image::launch::from_session(session, &PcCore, "Catalogue".into()).unwrap();
        let prepared = contract_options(prepared.prepared);
        for _ in 0..2 {
            let mut machine = core_launch::start(&prepared).unwrap();
            assert!(journal.is_file());
            assert!(!owner.path().join("media/a.upfj").exists());
            assert!(journal_files::offline::JournalTool::open(
                &base_path,
                &journal,
                journal_files::offline::Access::Mutate
            )
            .is_err());
            machine.run(RunLimit::steps(8300)).unwrap();
            machine.finish().unwrap();
        }
        assert_eq!(std::fs::read(base_path).unwrap(), vec![0xf6; 368640]);
        assert!(journal_files::offline::JournalTool::open(
            &temp.path().join("external.img"),
            &journal,
            journal_files::offline::Access::Mutate
        )
        .is_ok());
    });
}

#[test]
fn library_start_matches_verified_file_sessions_and_releases_finished_records() {
    use cpu::Cpu as _;
    use pc_library::library_worker::{Event, Finish, Recovery, Request, Worker, WorkspaceKind};
    use std::{fs, sync::Arc, time::Duration};
    let temp = machine_file::test_support::TempDir::new("library-start-comparison");
    let Some(bios) = super::sound_levels_tests::open_fixture(
        "vendor/unemu-at-bios/bios-unemu-at.bin",
        "vendor/unemu-at-bios/fetch.sh",
    ) else {
        return;
    };
    fs::copy(bios, temp.path().join("bios.bin")).unwrap();
    fs::write(temp.path().join("disk.img"), vec![0xf6; 360 * 1024]).unwrap();
    fs::write(temp.path().join("cmos.bin"), [0; 64]).unwrap();
    let path = temp.path().join("machine.toml");
    fs::write(&path, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[board.rtc.nvram]\nimage='cmos.bin'\nwrite='journal'\njournal='cmos.upfj'\n[[card]]\nkind='vga-text'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\n[[card.drives.media]]\nimage='disk.img'\nwrite='journal'\njournal='disk.upfj'\n").unwrap();
    let store = image_store::Store::open(temp.path().join("store")).unwrap();
    let image = pc_image::import_machine(&store, &path, "test:library", &Default::default())
        .unwrap()
        .manifest;
    let row = Recovery {
        schema: 1,
        kind: WorkspaceKind::Regular,
        reference: "test:library".into(),
        image: image.clone(),
        title: "Library comparison".into(),
        recovery: false,
        machine: None,
        name: String::new(),
    }
    .row();
    let mut worker = Worker::spawn(store.root().to_owned(), Arc::new(|| {}));
    let receive = |predicate: &dyn Fn(&Event) -> bool| loop {
        let event = worker
            .receive
            .recv_timeout(Duration::from_secs(60))
            .unwrap();
        if let Event::Error(error) = &event {
            panic!("worker: {error}");
        }
        if predicate(&event) {
            break event;
        }
    };
    receive(&|e| matches!(e, Event::Recoveries(_)));
    let old = store.workspace("comparison").unwrap();
    for cycle in 0..2 {
        worker
            .send
            .send(Request::Launch(row.clone(), WorkspaceKind::Regular, None))
            .unwrap();
        let Event::Launched(start, _, name, _, _) = receive(&|e| matches!(e, Event::Launched(..)))
        else {
            unreachable!()
        };
        let prepared = start
            .launch
            .prepared
            .with_options(vec![("speaker-trace".into(), "true".into())]);
        let mut created = core_launch::start_with(&prepared, |r| PcCore.create_machine(r)).unwrap();
        worker.send.send(Request::Assembled).unwrap();
        let session = pc_image::workspace::prepare_pinned(&old, &image, false).unwrap();
        let assembled = machine_file::assemble_with_verified_bases(
            &session.machine,
            &policy(),
            Default::default(),
            &session.bases,
        )
        .unwrap();
        let mut direct = PcMachine::from_assembled(&session.machine, assembled).unwrap();
        created.set_outputs(Outputs::NONE.with_audio(true));
        direct.set_outputs(Outputs::NONE.with_audio(true));
        for steps in [100, 200, 100_000] {
            assert_eq!(
                created.run(RunLimit::steps(steps)),
                direct.run(RunLimit::steps(steps))
            );
            assert_eq!(
                (
                    created.now(),
                    created.steps(),
                    created.assembled_ref().machine.cpu().retired()
                ),
                (
                    direct.now(),
                    direct.steps(),
                    direct.assembled_ref().machine.cpu().retired()
                )
            );
            for display in [DisplayId(0), DisplayId(1)] {
                let left = created.frame(display).unwrap();
                let right = direct.frame(display).unwrap();
                assert_eq!(
                    (
                        left.width,
                        left.height,
                        left.aspect,
                        left.visible,
                        left.pixels
                    ),
                    (
                        right.width,
                        right.height,
                        right.aspect,
                        right.visible,
                        right.pixels
                    )
                );
            }
            let (mut left, mut right) = (Vec::new(), Vec::new());
            created.drain_audio(&mut left).unwrap();
            direct.drain_audio(&mut right).unwrap();
            assert_eq!(left, right);
            assert_eq!(created.slot_states(), direct.slot_states());
            let (mut left, mut right) = (Vec::new(), Vec::new());
            created.drain_messages(&mut left);
            direct.drain_messages(&mut right);
            assert_eq!(left, right);
        }
        assert_eq!(created.finish().unwrap(), direct.finish().unwrap());
        // Finishing releases journals even while the old machine is still alive.
        let reopened = core_launch::start(&prepared).unwrap();
        drop(reopened);
        for record in &session.launch.as_ref().unwrap().document().records {
            assert_eq!(
                fs::read(old.path().join("media").join(&record.name)).unwrap(),
                fs::read(
                    store
                        .workspace_path(&name)
                        .unwrap()
                        .join("media")
                        .join(&record.name)
                )
                .unwrap(),
                "{}, cycle {cycle}",
                record.name
            );
        }
        drop(created);
        drop(direct);
        worker.send.send(Request::Finish(Finish::Retain)).unwrap();
        receive(&|e| matches!(e, Event::Finished));
    }
    worker.shutdown();
}

#[test]
fn an_unfinished_stored_machine_releases_its_records_on_drop() {
    pc_image::test_support::with_synthetic_firmware(|| {
        let (_temp, store, _) = stored_fixture();
        let owner = store.workspace("unfinished").unwrap();
        let prepared = pc_image::launch::prepare_workspace(
            &owner,
            "test:launch",
            true,
            &PcCore,
            "Unfinished".into(),
        )
        .unwrap();
        let prepared = contract_options(prepared.prepared);
        let mut machine = core_launch::start(&prepared).unwrap();
        machine.run(RunLimit::steps(8300)).unwrap();
        assert_eq!(
            core_launch::start(&prepared).err().unwrap().kind,
            ErrorKind::Usage
        );
        drop(machine);
        let mut next = core_launch::start(&prepared).unwrap();
        next.run(RunLimit::steps(8300)).unwrap();
        next.finish().unwrap();
    });
}
