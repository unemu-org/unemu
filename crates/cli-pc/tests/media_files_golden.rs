// SPDX-License-Identifier: AGPL-3.0-or-later
//! FreeDOS reads diskettes that `media create floppy --add` built from host
//! trees, through the BIOS and the floppy controller, from drive B:: one disk
//! listed and typed, and a two-disk split set entered with CD and rebuilt with
//! XCOPY. Microsoft FAT Specification, BPB, FAT12 entries and FAT Directory
//! Structure; IBM PC AT Technical Reference, diskette drive adapter. See
//! machines/README.md § Preparing disks.

use assembly::{
    ConsoleProbe, DeviceId, FloppyAction, FloppyMediaList, FloppyMedium, ImageSource,
    InjectionSchedule, KeyboardStimulus, ObservationPolicy, ObservationRequest, OutputKind,
    RunPolicy, StopCondition,
};
use machine::RunOutcome;
use std::ffi::OsString;
use video::Frame;

#[path = "../../pc-harness/tests/support/fat12.rs"]
mod fat12;
#[path = "../../pc-harness/tests/support/fixtures.rs"]
mod fixtures;

/// Set-1 make/break codes for a FreeDOS command line: lowercase letters,
/// digits, space, period, backslash, slash, colon (shifted) and Enter.
fn keys(text: &str) -> Vec<u8> {
    let mut keys = Vec::new();
    for byte in text.bytes() {
        let (code, shift) = match byte {
            b'a'..=b'z' => (
                [
                    0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
                    0x31, 0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
                ][usize::from(byte - b'a')],
                false,
            ),
            b'1'..=b'9' => (byte - b'1' + 2, false),
            b'0' => (0x0b, false),
            b' ' => (0x39, false),
            b'.' => (0x34, false),
            b'\\' => (0x2b, false),
            b'/' => (0x35, false),
            b':' => (0x27, true),
            b'\r' => (0x1c, false),
            _ => panic!("unsupported script key {byte}"),
        };
        if shift {
            keys.push(0x2a);
        }
        keys.extend([code, code | 0x80]);
        if shift {
            keys.push(0xaa);
        }
    }
    keys
}

fn screen(assembled: &mut assembly::AssembledMachine) -> String {
    let capture = assembled.capture_text(DeviceId::Mda).unwrap();
    let Frame::Text(grid) = capture.frame else {
        panic!("expected the MDA text console");
    };
    grid.rows_as_strings().join("\n")
}

/// Run in 200,000-instruction steps until the screen satisfies `done`. The
/// cap leaves room for XCOPY to read two 720K disks at the drive's data rate.
fn run_until(assembled: &mut assembly::AssembledMachine, done: impl Fn(&str) -> bool) -> String {
    let cap = 4_000_000_000;
    loop {
        let text = screen(assembled);
        if done(&text) {
            return text;
        }
        let next = assembled.machine.virtual_time() + 200_000;
        assert!(next < cap, "budget exhausted:\n{text}");
        let outcome = assembled
            .run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
        assert_eq!(outcome, RunOutcome::RetiredCapReached, "\n{text}");
    }
}

/// Type a command one key at a time so the BIOS type-ahead ring never fills.
fn type_line(assembled: &mut assembly::AssembledMachine, keyboard: usize, line: &str) {
    for character in format!("{line}\r").chars() {
        assembly::inject_scancodes(
            &mut assembled.machine,
            keyboard,
            &keys(&character.to_string()),
        );
        let next = assembled.machine.virtual_time() + 100_000;
        assembled
            .run_session(&RunPolicy::natural(
                Some(next),
                Some(pc_harness::BIOS_IDLE_LIMIT),
            ))
            .unwrap();
    }
}

/// `upcli` with these arguments must succeed.
fn upcli(args: &[OsString]) -> String {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    assert_eq!(
        cli_pc::command::run(args, &mut out, &mut err),
        0,
        "{}",
        String::from_utf8_lossy(&err)
    );
    String::from_utf8(out).unwrap()
}

/// The reference FreeDOS machine (Xi8088 BIOS, MDA console) with `b` as a
/// 1.44M B: drive, booted past the date and time prompts to `A:\>`.
fn boot(a: Option<Vec<u8>>, b: FloppyMediaList) -> (assembly::AssembledMachine, usize) {
    let mut config = machine_file::load(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml"),
    )
    .unwrap();
    let mut second = config.drives[0].clone();
    if let Some(image) = a {
        config.drives[0].image = ImageSource::Inline(image);
        config.drives[0].expected_sha256 = None;
    }
    second.unit = 1;
    second.image = b.entries[0].image.clone();
    second.expected_sha256 = None;
    second.write = assembly::WriteMode::ReadOnly;
    second.media = Some(b);
    config.drives.push(second);
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..Default::default()
    };
    let mut assembled = assembly::assemble_with_observation(&config, &policy).unwrap();
    let keyboard = assembled.device_index[&DeviceId::Keyboard];
    assembled
        .run_session(&RunPolicy {
            retired_cap: Some(pc_harness::BIOS_MAX_RETIRED),
            idle_limit: Some(pc_harness::BIOS_IDLE_LIMIT),
            stop: StopCondition::PromptThen {
                console: ConsoleProbe {
                    device: DeviceId::Mda,
                },
                needle: pc_harness::DOS_DATE_PROMPT.into(),
                poll_step: pc_harness::DOS_INTERACTIVE_POLL_STEP,
                then_budget: 0,
            },
            stimuli: vec![KeyboardStimulus {
                keyboard_index: keyboard,
                scancodes: [pc_harness::KEY_ENTER, pc_harness::KEY_ENTER].concat(),
                schedule: InjectionSchedule::AfterPrompt,
            }],
        })
        .unwrap();
    run_until(&mut assembled, |text| at(text, "A:\\>"));
    (assembled, keyboard)
}

/// The last screen line is `prompt`: the previous command has finished.
fn at(text: &str, prompt: &str) -> bool {
    text.trim_end().ends_with(prompt)
}

fn read_only(name: &str, image: Vec<u8>) -> FloppyMedium {
    FloppyMedium {
        image: ImageSource::Inline(image),
        expected_sha256: None,
        write: assembly::WriteMode::ReadOnly,
        name: name.into(),
    }
}

/// Type `line`, then run until the screen ends with `prompt` again.
fn command(
    assembled: &mut assembly::AssembledMachine,
    keyboard: usize,
    line: &str,
    prompt: &str,
) -> String {
    type_line(assembled, keyboard, line);
    run_until(assembled, |text| at(text, prompt))
}

#[ignore = "slow: boots FreeDOS, then lists and types files from a disk built from host files"]
#[test]
fn freedos_reads_a_floppy_built_from_host_files() {
    assert!(include_str!("../../../Cargo.lock")
        .contains("rev=7a9ea390b8f588ce4af9845980bbf8534f793f61"));
    let (Some(_), Some(_)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
    ) else {
        return;
    };
    let tree = assembly::test_support::TempDir::new("host-tree-floppy");
    let host = tree.path().join("host");
    std::fs::create_dir_all(host.join("sub")).unwrap();
    std::fs::write(host.join("hello.txt"), b"hello from the host\r\n").unwrap();
    std::fs::write(host.join("sub/file.txt"), b"inside a host folder\r\n").unwrap();
    let image_path = tree.path().join("host.img");
    let args: Vec<OsString> = vec![
        "media".into(),
        "create".into(),
        "floppy".into(),
        "--type".into(),
        "1.44M".into(),
        "--add".into(),
        host.join("hello.txt").into(),
        "--add".into(),
        host.join("sub").into(),
        "--label".into(),
        "HOSTFILES".into(),
        image_path.clone().into(),
    ];
    upcli(&args);
    let image = std::fs::read(&image_path).unwrap();
    let (mut assembled, keyboard) = boot(
        None,
        FloppyMediaList {
            entries: vec![read_only("host.img", image)],
            empty: false,
            inserted: 0,
        },
    );
    let prompt = |text: &str| at(text, "A:\\>");

    type_line(&mut assembled, keyboard, "dir b:\\");
    run_until(&mut assembled, |text| {
        text.contains("bytes free") && prompt(text)
    });
    type_line(&mut assembled, keyboard, "type b:\\sub\\file.txt");
    let text = run_until(&mut assembled, |text| {
        text.contains("inside a host folder") && prompt(text)
    });
    eprintln!("host-tree floppy in B:\n{text}");
    // The fixed serial, FAT epoch dates and free space of the built disk:
    // 2,847 clusters less one each for HELLO.TXT, SUB and SUB\FILE.TXT.
    let session = &text[text.find("A:\\>dir b:\\").expect("DIR command echo")..];
    let lines: Vec<&str> = session.trim_end().lines().map(str::trim_end).collect();
    assert_eq!(
        lines,
        [
            "A:\\>dir b:\\",
            " Volume in drive B is HOSTFILES",
            " Volume Serial Number is 2895-9855",
            " Directory of B:\\",
            "",
            "HELLO    TXT            21  01-01-80 12:00a",
            "SUB                  <DIR>  01-01-80 12:00a",
            "         1 file(s)             21 bytes",
            "         1 dir(s)       1,456,128 bytes free",
            "A:\\>type b:\\sub\\file.txt",
            "inside a host folder",
            "A:\\>",
        ],
        "\n{text}"
    );
    assembled.finish_persistence().unwrap();
}

/// The main use of `--split`: a tree larger than one disk goes onto a set,
/// every disk under the same top folder, and XCOPY from each disk in turn
/// rebuilds it. The guest also enters a second-level folder with CD and lists
/// a folder that spans two clusters.
///
/// The Xi8088 profile has no hard disk, and its FreeDOS floppy holds only the
/// kernel and shell, so XCOPY.EXE comes from the open tools floppy and the
/// tree is rebuilt on the writable in-memory A:.
#[ignore = "slow: boots FreeDOS, enters and XCOPYs a two-disk 720K set built from host files"]
#[test]
fn freedos_enters_and_rebuilds_a_split_set_with_xcopy() {
    assert!(include_str!("../../../Cargo.lock")
        .contains("rev=7a9ea390b8f588ce4af9845980bbf8534f793f61"));
    let (Some(_), Some(mut boot_disk), Some(tools)) = (
        fixtures::fixture("kiselev-bios/bios-xi8088.bin"),
        fixtures::fixture("freedos/freedos.img"),
        fixtures::fixture("freedos/freedos-tools-1200.img"),
    ) else {
        return;
    };
    let tools_pin = fixtures::parse_pin(
        include_str!("../../../vendor/freedos/fetch.sh"),
        "check_sha256 \"$OUT_TOOLS\" \"",
    )
    .unwrap();
    assert_eq!(assembly::test_support::fixture_sha256(&tools), tools_pin);
    let xcopy = fat12::file(&tools, b"XCOPY   EXE").expect("XCOPY.EXE on the tools floppy");
    fat12::add_file(&mut boot_disk, b"XCOPY   EXE", &xcopy);

    // 400,000 bytes, then forty 9,000-byte files: the first disk takes the
    // large file and 35 of them (37 entries, two 1,024-byte clusters of
    // folder), the second the last five and SUB\SUB2.
    let tree = assembly::test_support::TempDir::new("host-tree-split");
    let host = tree.path().join("host");
    std::fs::create_dir_all(host.join("many")).unwrap();
    std::fs::create_dir_all(host.join("sub/sub2")).unwrap();
    let pattern = |length: usize, seed: u8| -> Vec<u8> {
        (0..length)
            .map(|i| (i as u8).wrapping_mul(29).wrapping_add(seed))
            .collect()
    };
    std::fs::write(host.join("big.dat"), pattern(400_000, 0)).unwrap();
    for index in 1..=40u8 {
        std::fs::write(
            host.join(format!("many/f{index:02}.dat")),
            pattern(9_000, index),
        )
        .unwrap();
    }
    std::fs::write(host.join("sub/sub2/deep.txt"), b"deep in a split set\r\n").unwrap();
    let set = tree.path().join("set.img");
    let mut args: Vec<OsString> = ["media", "create", "floppy", "--type", "720K"]
        .map(OsString::from)
        .into();
    for source in ["big.dat", "many", "sub"] {
        args.extend(["--add".into(), host.join(source).into()]);
    }
    args.extend(["--top", "TOP", "--label", "SPLIT", "--split"].map(OsString::from));
    args.push(set.into());
    let output = upcli(&args);
    assert!(
        output.contains("label SPLIT 1, 36 files, 4096 bytes free"),
        "{output}"
    );
    assert!(output.contains("label SPLIT 2, 6 files"), "{output}");
    let disk = |k: usize| std::fs::read(tree.path().join(format!("set-{k}of2.img"))).unwrap();

    let (mut assembled, keyboard) = boot(
        Some(boot_disk),
        FloppyMediaList {
            entries: vec![
                read_only("set-1of2.img", disk(1)),
                read_only("set-2of2.img", disk(2)),
            ],
            empty: false,
            inserted: 0,
        },
    );
    let a = "A:\\>";
    let listing = command(&mut assembled, keyboard, "dir b:\\top", a);
    eprintln!("disk 1:\n{listing}");
    for line in [
        " Volume in drive B is SPLIT 1",
        " Directory of B:\\TOP",
        "BIG      DAT       400,000  01-01-80 12:00a",
        "MANY                 <DIR>  01-01-80 12:00a",
    ] {
        assert!(listing.contains(line), "{line:?}:\n{listing}");
    }
    // The listing scrolls; its end shows entries 33 to 37 of the folder, which
    // live in its second cluster.
    let listing = command(&mut assembled, keyboard, "dir b:\\top\\many", a);
    eprintln!("disk 1, multi-cluster folder:\n{listing}");
    for line in [
        "F31      DAT         9,000  01-01-80 12:00a",
        "F35      DAT         9,000  01-01-80 12:00a",
        "        35 file(s)        315,000 bytes",
        "         2 dir(s)           4,096 bytes free",
    ] {
        assert!(listing.contains(line), "{line:?}:\n{listing}");
    }
    command(&mut assembled, keyboard, "md a:\\top", a);
    let copied = command(&mut assembled, keyboard, "xcopy b:\\top a:\\top /s /e", a);
    eprintln!("XCOPY from disk 1:\n{copied}");
    assert!(
        copied.trim_end().ends_with("36 file(s) copied\nA:\\>"),
        "{copied}"
    );

    assembled
        .change_floppy(DeviceId::Fdc, 1, FloppyAction::Insert(1))
        .unwrap();
    command(&mut assembled, keyboard, "cd b:\\top\\sub\\sub2", a);
    command(&mut assembled, keyboard, "b:", "B:\\TOP\\SUB\\SUB2>");
    command(&mut assembled, keyboard, "cd ..", "B:\\TOP\\SUB>");
    let listing = command(&mut assembled, keyboard, "dir", "B:\\TOP\\SUB>");
    eprintln!("disk 2 after CD:\n{listing}");
    for line in [
        " Volume in drive B is SPLIT 2",
        " Directory of B:\\TOP\\SUB",
        "SUB2                 <DIR>  01-01-80 12:00a",
        "         3 dir(s)         678,912 bytes free",
    ] {
        assert!(listing.contains(line), "{line:?}:\n{listing}");
    }
    command(&mut assembled, keyboard, "a:", a);
    let copied = command(&mut assembled, keyboard, "xcopy b:\\top a:\\top /s /e", a);
    eprintln!("XCOPY from disk 2:\n{copied}");
    assert!(
        copied.trim_end().ends_with("6 file(s) copied\nA:\\>"),
        "{copied}"
    );

    let rebuilt = command(&mut assembled, keyboard, "dir a:\\top\\many", a);
    eprintln!("rebuilt folder:\n{rebuilt}");
    for line in [
        "F40      DAT         9,000",
        "        40 file(s)        360,000 bytes",
    ] {
        assert!(rebuilt.contains(line), "{line:?}:\n{rebuilt}");
    }
    let rebuilt = command(&mut assembled, keyboard, "dir a:\\top", a);
    assert!(rebuilt.contains("BIG      DAT       400,000"), "{rebuilt}");
    let typed = command(
        &mut assembled,
        keyboard,
        "type a:\\top\\sub\\sub2\\deep.txt",
        a,
    );
    assert!(
        typed.trim_end().ends_with("deep in a split set\nA:\\>"),
        "{typed}"
    );
    assembled.finish_persistence().unwrap();
}
