// SPDX-License-Identifier: AGPL-3.0-or-later
//! A machine started from a diskette `media create` built runs the disk's
//! own boot sector, as from a DOS-formatted data disk: it shows that this is
//! not a system disk, waits for a key, and after the key the BIOS loads the
//! boot sector again (INT 19h), which shows the message a second time. Run on
//! both open BIOSes with the MDA console and no option ROM, so the BIOS's own
//! INT 19h does the retry: the Xi8088 BIOS of the reference FreeDOS machine,
//! and the same machine with the open AT BIOS. IBM Personal Computer AT Technical
//! Reference, BIOS bootstrap loader, INT 16h and INT 19h; the boot code is
//! crates/media/src/fat12/boot.asm.

use assembly::{
    AssembledMachine, DeviceId, ImageSource, ObservationPolicy, ObservationRequest, OutputKind,
    RunPolicy,
};
use std::ffi::OsString;
use video::Frame;

use pc_harness::test_support::fixtures;

// Moving the core pin requires checking the recorded golden again.
const CORE_REV: &str = "0f2795402e947e235a7b892c931910b05f05d61f";

#[path = "../../pc-image/tests/support/core_pin.rs"]
mod core_pin;
use core_pin::assert_core_revision;

#[test]
fn golden_core_revision_matches_lock() {
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
}

const MESSAGE: &str = "This is not a system disk.";
const PROMPT: &str = "Replace it and press any key.";

fn screen(assembled: &mut AssembledMachine) -> String {
    let Frame::Text(grid) = assembled.capture_text(DeviceId::Mda).unwrap().frame else {
        panic!("expected the MDA text console");
    };
    grid.rows_as_strings().join("\n")
}

/// Run in 200,000-instruction steps until the screen satisfies `done`, or
/// until `budget` more virtual time has passed; return the last screen.
fn run_until(assembled: &mut AssembledMachine, budget: u64, done: impl Fn(&str) -> bool) -> String {
    let cap = assembled.machine.virtual_time() + budget;
    loop {
        let text = screen(assembled);
        if done(&text) || assembled.machine.virtual_time() >= cap {
            return text;
        }
        let before = assembled.machine.virtual_time();
        let next = (before + 200_000).min(cap);
        assembled
            .run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
        if assembled.machine.virtual_time() == before {
            return screen(assembled);
        }
    }
}

/// A 1.44M data disk built from one host file by `unemu media create`.
fn data_disk() -> Vec<u8> {
    let scratch = machine_file::test_support::TempDir::new("boot-sector-golden");
    let file = scratch.path().join("readme.txt");
    std::fs::write(&file, b"no system here\r\n").unwrap();
    let image = scratch.path().join("data.img");
    let args: Vec<OsString> = ["media", "create", "floppy", "--type", "1.44M", "--add"]
        .into_iter()
        .map(OsString::from)
        .chain([file.into_os_string(), image.clone().into_os_string()])
        .collect();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    assert_eq!(
        cli_pc::command::run(&args, &mut output, &mut errors),
        0,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    std::fs::read(image).unwrap()
}

fn boots_the_data_disk_and_retries_after_a_key(profile: &std::path::Path, firmware: &[&str]) {
    assert_core_revision(include_str!("../../../Cargo.lock"), CORE_REV);
    for fixture in firmware {
        if fixtures::fixture(fixture).is_none() {
            return;
        }
    }
    let mut config = machine_file::load(profile).unwrap();
    let profile = profile.file_name().unwrap().to_string_lossy();
    let floppy = config
        .drives
        .iter_mut()
        .find(|drive| drive.controller == DeviceId::Fdc && drive.unit == 0)
        .unwrap();
    floppy.image = ImageSource::Inline(data_disk());
    floppy.expected_sha256 = None;
    floppy.write = assembly::WriteMode::ReadOnly;
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut assembled = machine_file::assemble_with_observation(&config, &policy).unwrap();
    let keyboard = assembled.device_index[&DeviceId::Keyboard];
    let shown = |count: usize| move |text: &str| text.matches(PROMPT).count() >= count;

    let first = run_until(&mut assembled, pc_harness::BIOS_MAX_RETIRED * 4, shown(1));
    assert_eq!(first.matches(MESSAGE).count(), 1, "{profile}:\n{first}");
    assert_eq!(first.matches(PROMPT).count(), 1, "{profile}:\n{first}");

    // Without a key the disk waits: nothing more is printed.
    let waiting = run_until(&mut assembled, 5_000_000, shown(2));
    assert_eq!(waiting.matches(PROMPT).count(), 1, "{profile}:\n{waiting}");

    assembly::inject_scancodes(&mut assembled.machine, keyboard, &pc_harness::KEY_ENTER);
    let again = run_until(&mut assembled, pc_harness::BIOS_MAX_RETIRED, shown(2));
    assert_eq!(again.matches(MESSAGE).count(), 2, "{profile}:\n{again}");
    assert_eq!(again.matches(PROMPT).count(), 2, "{profile}:\n{again}");
    eprintln!("{profile}:\n{again}");
}

fn machines() -> std::path::PathBuf {
    pc_harness::test_support::root()
        .join("machines")
        .canonicalize()
        .unwrap()
}

#[test]
fn xi8088_bios_runs_the_data_disk_boot_sector_and_retries() {
    boots_the_data_disk_and_retries_after_a_key(
        &machines().join("at-freedos.toml"),
        &["kiselev-bios/bios-xi8088.bin"],
    );
}

/// The IBM 5170 preset with the open AT BIOS, an MDA console and one 1.44M
/// drive, and no option ROM: no XTIDE, whose own INT 19h would otherwise do
/// the retry.
#[test]
fn open_at_bios_runs_the_data_disk_boot_sector_and_retries() {
    let profile = machine_file::test_support::TempDir::new("boot-sector-open-at");
    let file = profile.path().join("at-open-floppy.toml");
    std::fs::write(profile.path().join("data.img"), data_disk()).unwrap();
    let bios = machines().join("../vendor/unemu-at-bios/bios-unemu-at.bin");
    let text = format!(
        "schema = 1\nextends = \"preset:ibm-5170\"\nname = \"at-open-floppy\"\n\n\
         [board]\n\
         # The open AT BIOS reads the switch with the IBM polarity: mono keeps the MDA.\n\
         display_switch = \"mono\"\n\n\
         [firmware.bios]\nimage = {:?}\n\
         sha256 = \"6938dc8b665fa120f33775163ab6bacc559389525db87adedf734416f9ef2442\"\n\n\
         [[card]]\nid = \"fdc-765\"\n\n[[card.drives]]\nunit = 0\ntype = \"1.44M\"\n\
         image = \"data.img\"\n",
        bios,
    );
    std::fs::write(&file, text).unwrap();
    boots_the_data_disk_and_retries_after_a_key(&file, &["unemu-at-bios/bios-unemu-at.bin"]);
}
