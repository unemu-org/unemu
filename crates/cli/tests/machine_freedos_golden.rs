// SPDX-License-Identifier: AGPL-3.0-or-later
//! FreeDOS through a machine: the machine's floppy journal keeps a file the
//! guest wrote across two starts, and Reset to image… drops it; see
//! machine-images §3.2 and persistent-media.md §Integration. The guest writes
//! through FreeCOM, INT 13h and the µPD765 WRITE DATA command onto FAT12
//! (Microsoft FAT Specification, FAT12 BPB and Directory Structure).
//!
//! The image is the reference `at-freedos` machine with its floppy journaled
//! instead of volatile: the Kiselev Xi8088 BIOS and the FreeDOS 1.44 MB disk.

use assembly::{AssembledMachine, DeviceId};
use pc_image::machine;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
use video::Frame;

#[allow(dead_code)]
#[path = "support/fat12.rs"]
mod fat12;
#[path = "support/fixtures.rs"]
mod fixtures;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn console(machine: &AssembledMachine) -> String {
    let Frame::Text(grid) = machine.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("MDA text required")
    };
    grid.rows_as_strings().join("\n")
}
fn keys(text: &str) -> Vec<u8> {
    let mut keys = Vec::new();
    for byte in text.bytes() {
        let (code, shift) = match byte {
            b'e' => (0x12, false),
            b'c' => (0x2E, false),
            b'h' => (0x23, false),
            b'o' => (0x18, false),
            b'x' => (0x2D, false),
            b'z' => (0x2C, false),
            b't' => (0x14, false),
            b'y' => (0x15, false),
            b'p' => (0x19, false),
            b' ' => (0x39, false),
            b'\r' => (0x1C, false),
            b'>' => (0x34, true),
            _ => panic!("unsupported key"),
        };
        if shift {
            keys.push(0x2A);
        }
        keys.extend([code, code | 0x80]);
        if shift {
            keys.push(0xAA);
        }
    }
    keys
}

/// Start the machine, boot FreeDOS, type `command` and finish its journals.
fn session(store: &image_store::Store, id: &str, command: &str) -> String {
    let owner = machine::own(store, id).unwrap();
    let started = machine::prepare(&owner).unwrap();
    let observation = assembly::ObservationPolicy {
        routes: vec![assembly::ObservationRequest {
            device: DeviceId::Mda,
            output: assembly::OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut assembled = assembly::assemble_with_verified_bases(
        &started.session.machine,
        &observation,
        assembly::AssemblyOptions::default(),
        &started.session.bases,
    )
    .unwrap();
    machine::confirm(&owner, &started.inserted, Some("golden".into())).unwrap();
    let keyboard_index = assembled.device_index[&DeviceId::Keyboard];
    let policy = assembly::RunPolicy {
        retired_cap: Some(100_000_000),
        idle_limit: Some(cli::BIOS_IDLE_LIMIT),
        stop: assembly::StopCondition::PromptThen {
            console: assembly::ConsoleProbe {
                device: DeviceId::Mda,
            },
            needle: cli::DOS_DATE_PROMPT.into(),
            poll_step: cli::DOS_INTERACTIVE_POLL_STEP,
            then_budget: 10_000_000,
        },
        stimuli: vec![assembly::KeyboardStimulus {
            keyboard_index,
            scancodes: keys(&format!("\r\r{command}\r")),
            schedule: assembly::InjectionSchedule::AfterPrompt,
        }],
    };
    assembled.run_session(&policy).unwrap();
    let text = console(&assembled);
    eprintln!("{text}");
    assert!(
        text.lines().any(|row| row.trim() == "A:\\>"),
        "the command must return to the prompt"
    );
    assembled.finish_persistence().unwrap();
    text
}

#[test]
#[ignore = "slow: two BIOS/FreeDOS boots through a machine and its floppy journal"]
fn freedos_writes_persist_in_a_machine_until_reset() {
    let (Some(_), Some(floppy)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let temp = Temp(std::env::temp_dir().join(format!(
        "machine-freedos-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    std::fs::create_dir(&temp.0).unwrap();
    let machines = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../machines");
    let reference = std::fs::read_to_string(machines.join("at-freedos.toml")).unwrap();
    assert_eq!(reference.matches("write = \"volatile\"").count(), 1);
    let vendor = machines.join("../vendor/");
    let file = temp.0.join("at-freedos-journal.toml");
    std::fs::write(
        &file,
        reference
            .replace("../vendor/", vendor.to_str().unwrap())
            .replace(
                "write = \"volatile\"",
                "write = \"journal\"\njournal = \"source.upfj\"",
            ),
    )
    .unwrap();
    let store = image_store::Store::open(temp.0.join("store")).unwrap();
    pc_image::import_machine(
        &store,
        &file,
        "freedos:1.0.0",
        &pc_image::ImportOptions {
            no_compress: true,
            ..Default::default()
        },
    )
    .unwrap();
    let id = machine::create_from_tag(&store, "freedos:1.0.0", Some("FreeDOS"), None).unwrap();
    let journal = store
        .workspace_path(&id)
        .unwrap()
        .join("media/medium-0.upfj");
    let replay = || {
        let scan = media::scan(
            &std::fs::read(&journal).unwrap(),
            &floppy,
            floppy.len() as u64,
        );
        assert_eq!(scan.report.status, media::Status::Valid);
        scan.image.unwrap()
    };

    let first = session(&store, &id, "echo x>zz");
    assert!(first.contains("A:\\>echo x>zz"));
    assert_eq!(
        fat12::file(&replay(), b"ZZ         ").as_deref(),
        Some(&b"x\r\n"[..])
    );
    let listing = machine::get(&store, &id).unwrap();
    assert_eq!(listing.state.to_string(), "Changed: A:");
    assert_eq!(listing.machine.last_run.as_deref(), Some("golden"));

    // The second start opens the same journal: the file is still there.
    let second = session(&store, &id, "type zz");
    assert!(second.contains("A:\\>type zz"));
    assert!(second.lines().any(|row| row.trim() == "x"));
    assert_eq!(
        fat12::file(&replay(), b"ZZ         ").as_deref(),
        Some(&b"x\r\n"[..])
    );

    // Reset to image… drops the journal; the image never changed.
    machine::reset(&store, &id).unwrap();
    assert!(!journal.exists());
    assert_eq!(
        machine::get(&store, &id).unwrap().state.to_string(),
        "Same as image"
    );
    let owner = machine::own(&store, &id).unwrap();
    let started = machine::prepare(&owner).unwrap();
    let drive = &started.session.machine.drives[0];
    let medium = drive
        .media
        .as_ref()
        .map_or(&drive.image, |list| &list.entries[0].image);
    let assembly::ImageSource::File(path) = medium else {
        panic!("a stored floppy file expected")
    };
    let base = std::fs::read(path).unwrap();
    assert_eq!(base, floppy);
    assert!(fat12::file(&base, b"ZZ         ").is_none());
}
