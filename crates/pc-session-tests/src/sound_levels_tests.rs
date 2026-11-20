// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host mix levels through the desktop audio path: the presenter renders each
//! source into the window's host sink, and the tests read what the host
//! callback would play. See `backlog/docs/design/audio.md` §Host mix levels.
//!
//! Guest programs drive the PC speaker (IBM Personal Computer AT Technical
//! Reference, System Board, Speaker; Intel 8254 datasheet, Mode 3), the SB16
//! direct DAC (Creative Sound Blaster Series Hardware Programming Guide, §6
//! command 10h) and one YMF262 channel (Yamaha YMF262 datasheet, Register Map)
//! with the card's mixer left at its power-on levels.

use super::*;
use cpu::Cpu as _;

/// Guest machine code for the level programs.
#[derive(Default)]
struct Code(Vec<u8>);

impl Code {
    /// `mov dx, port; mov al, byte; out dx, al`.
    fn out(&mut self, port: u16, byte: u8) {
        self.0
            .extend_from_slice(&[0xBA, port as u8, (port >> 8) as u8, 0xB0, byte, 0xEE]);
    }

    /// PIT counter 2 at divisor 1193 (about 1 kHz) and port 61h bits 0-1 on.
    fn speaker_on(&mut self) {
        for (port, byte) in [(0x43, 0xB6), (0x42, 0xA9), (0x42, 0x04), (0x61, 0x03)] {
            self.out(port, byte);
        }
    }

    /// One 2-operator channel keyed on, its carrier sustained at full output.
    fn fm_tone(&mut self) {
        for (index, value) in [
            (0x20, 0x21),
            (0x23, 0x21),
            (0x40, 0x10),
            (0x43, 0x00),
            (0x60, 0xF4),
            (0x63, 0xF4),
            (0x80, 0x05),
            (0x83, 0x05),
            (0xC0, 0x30),
            (0xA0, 0x98),
            (0xB0, 0x31),
        ] {
            self.out(0x388, index);
            self.out(0x389, value);
        }
    }

    /// One period of a full-scale direct-DAC square: FFh, then 00h, each held
    /// for a counted `loop`.
    fn voice_square_period(&mut self, hold: u16) {
        for byte in [0xFF, 0x00] {
            self.out(0x22C, 0x10);
            self.out(0x22C, byte);
            self.0
                .extend_from_slice(&[0xB9, hold as u8, (hold >> 8) as u8, 0xE2, 0xFE]);
        }
    }

    /// A short backward or forward jump to `target`.
    fn jump(&mut self, opcode: u8, target: usize) {
        let offset = target as isize - (self.0.len() as isize + 2);
        self.0.push(opcode);
        self.0.push(i8::try_from(offset).unwrap() as u8);
    }
}

fn peak(frames: &[audio::StereoFrame]) -> i32 {
    frames
        .iter()
        .flatten()
        .map(|&level| i32::from(level).abs())
        .max()
        .unwrap_or(0)
}

fn dbfs(peak: i32) -> f64 {
    20.0 * (f64::from(peak) / 32768.0).log10()
}

/// How far the speaker's audible swing sits under the voice square's. The
/// speaker is a one-sided 0/+level square, so it swings half its peak about
/// its mean; the voice square swings its full peak about silence.
fn speaker_swing_below_voice(speaker: i32, voice: i32) -> f64 {
    dbfs(voice) - dbfs(speaker / 2)
}

/// The host sink's peak while a synthetic AT ROM sounds the chosen sources
/// forever, measured after the first 100 ms.
fn synthetic_peak(speaker: bool, fm: bool, voice: bool) -> i32 {
    let mut code = Code(vec![0xFA]);
    if speaker {
        code.speaker_on();
    }
    if fm {
        code.fm_tone();
    }
    let start = code.0.len();
    if voice {
        // About 1 kHz at one retired instruction per PIT clock.
        code.voice_square_period(600);
    }
    code.jump(0xEB, start);
    let mut rom = vec![0; 32768];
    rom[..code.0.len()].copy_from_slice(&code.0);
    rom[32752..32757].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
    let mut config = assembly::bios(rom, Some(vec![0; 1_474_560]));
    config.clock.retired_per_pit_clock = 1;
    config.clock.retired_per_pit_clock_denominator = 1;
    let card = assembly::Sb16Cfg {
        base: 0x220,
        dma16: 5,
    };
    config.devices.push(
        assembly::DeviceConfig::new(
            DeviceId::Sb16,
            assembly::DeviceKind::Sb16(card),
            card.port_windows(),
        )
        .with_irq(5)
        .with_dma(assembly::DmaSpec {
            controller: DeviceId::Dma,
            channel: 1,
        }),
    );
    let mut presenter = new_presenter(
        &config,
        assembly::AssemblyOptions {
            allow_unlisted_firmware: true,
        },
        &mut io::sink(),
    )
    .unwrap();
    presenter.audio.activate_for_test();
    let mut level = 0;
    for ms in (10..=300).step_by(10) {
        presenter.advance_fully(Duration::from_millis(ms)).unwrap();
        let frames = presenter.audio.drain_for_test();
        if ms > 100 {
            level = level.max(peak(&frames));
        }
    }
    assert!(presenter.running && presenter.error.is_none());
    level
}

#[test]
fn desktop_sink_puts_a_full_scale_card_voice_at_minus_6_dbfs_and_the_speaker_12_db_below() {
    let speaker = synthetic_peak(true, false, false);
    let voice = synthetic_peak(false, false, true);
    let fm = synthetic_peak(false, true, false);
    let all = synthetic_peak(true, true, true);
    // A 00h byte is -32768 at the card; the host mix takes half.
    assert_eq!(voice, 16384, "card voice {:.1} dBFS", dbfs(voice));
    assert_eq!(speaker, 8192, "PC speaker {:.1} dBFS", dbfs(speaker));
    let below = speaker_swing_below_voice(speaker, voice);
    assert!(
        (10.0..=15.0).contains(&below),
        "speaker swing {below:.1} dB below the voice"
    );
    // One channel at full output peaks near 4084 natively; the host takes 3/4.
    assert!(
        (2800..=3100).contains(&fm),
        "FM channel {fm} ({:.1} dBFS)",
        dbfs(fm)
    );
    // Together they add up without reaching the mixer's saturation.
    assert!(
        (20000..i32::from(i16::MAX)).contains(&all),
        "all sources {all} ({:.1} dBFS)",
        dbfs(all)
    );
}

/// A boot sector for the open AT games machine: four phases of about a third
/// of a second each, numbered at 0000:0500 for the test, timed by the BIOS
/// tick count at 0040:006C. 1: PC speaker; 2: full-scale direct-DAC square;
/// 3: one FM channel; 4: all three; 5: done.
fn level_boot_sector() -> Vec<u8> {
    let mut code = Code(vec![0x31, 0xC0, 0x8E, 0xD8, 0xFB]); // xor ax,ax; mov ds,ax; sti
    let phase = |code: &mut Code, n: u8| code.0.extend_from_slice(&[0xC6, 6, 0, 5, n]);
    // mov bx,[046Ch]; add bx,ticks
    let deadline = |code: &mut Code, ticks: u8| {
        code.0
            .extend_from_slice(&[0x8B, 0x1E, 0x6C, 0x04, 0x83, 0xC3, ticks]);
    };
    // cmp [046Ch],bx; jne target
    let until = |code: &mut Code, target: usize| {
        code.0.extend_from_slice(&[0x39, 0x1E, 0x6C, 0x04]);
        code.jump(0x75, target);
    };
    let wait = |code: &mut Code, ticks: u8| {
        deadline(code, ticks);
        let here = code.0.len();
        until(code, here);
    };
    let voice = |code: &mut Code| {
        deadline(code, 6);
        let start = code.0.len();
        code.voice_square_period(200);
        until(code, start);
        code.out(0x22C, 0x10);
        code.out(0x22C, 0x80);
    };
    phase(&mut code, 1);
    code.speaker_on();
    wait(&mut code, 6);
    code.out(0x61, 0);
    wait(&mut code, 2);
    phase(&mut code, 2);
    voice(&mut code);
    wait(&mut code, 2);
    phase(&mut code, 3);
    code.fm_tone();
    wait(&mut code, 6);
    phase(&mut code, 4);
    code.speaker_on();
    voice(&mut code);
    code.out(0x61, 0);
    phase(&mut code, 5);
    let halt = code.0.len();
    code.0.push(0xF4);
    code.jump(0xEB, halt);
    let mut sector = vec![0; 1_228_800];
    sector[..code.0.len()].copy_from_slice(&code.0);
    sector[510..512].copy_from_slice(&[0x55, 0xAA]);
    sector
}

/// Run the level boot sector and return each phase's host-sink peak. Frames
/// from a host frame in which the phase changed belong to neither phase.
fn phase_peaks(mut presenter: Presenter) -> [i32; 5] {
    presenter.audio.activate_for_test();
    let mut peaks = [0; 5];
    let mut phase = 0;
    let mut now = Duration::ZERO;
    let bound = Duration::from_secs(60);
    while phase != 5 {
        assert!(now < bound, "the boot sector did not finish: phase {phase}");
        now += Duration::from_millis(10);
        presenter.advance_fully(now).unwrap();
        assert!(presenter.running && presenter.error.is_none());
        let frames = presenter.audio.drain_for_test();
        let cpu::DmaAccess::Complete(current) =
            presenter.assembled().machine.cpu().guest_dma_read(0x500)
        else {
            panic!("RAM at 500h is not memory");
        };
        if current == phase && (1..5).contains(&phase) {
            let slot = &mut peaks[usize::from(phase)];
            *slot = (*slot).max(peak(&frames));
        }
        phase = current;
    }
    peaks
}

fn assert_open_profile_levels(path: &str, peaks: [i32; 5]) {
    let [_, speaker, voice, fm, all] = peaks;
    assert_eq!(
        speaker,
        8192,
        "{path}: PC speaker {:.1} dBFS",
        dbfs(speaker)
    );
    let below = speaker_swing_below_voice(speaker, voice);
    assert!(
        (10.0..=15.0).contains(&below),
        "{path}: speaker swing {below:.1} dB below the voice"
    );
    assert_eq!(voice, 16384, "{path}: card voice {:.1} dBFS", dbfs(voice));
    assert!(
        (2800..=3100).contains(&fm),
        "{path}: FM channel {fm} ({:.1} dBFS)",
        dbfs(fm)
    );
    assert!(
        (20000..i32::from(i16::MAX)).contains(&all),
        "{path}: all sources {all} ({:.1} dBFS)",
        dbfs(all)
    );
}

pub(super) fn open_fixture(relative: &str, fetch: &str) -> Option<PathBuf> {
    let path = default_fixture(relative);
    if path.exists() {
        return Some(path);
    }
    let message = format!("missing fixture {} — run {fetch}", path.display());
    assert!(
        std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref() != Some(OsStr::new("1")),
        "{message}"
    );
    eprintln!("skipping: {message}");
    None
}

/// Write the level boot sector to `path` and make it A:'s only disk.
fn insert_level_disk(config: &mut assembly::MachineConfig, path: &Path) {
    std::fs::write(path, level_boot_sector()).unwrap();
    let drive = config
        .drives
        .iter_mut()
        .find(|drive| drive.controller == DeviceId::Fdc && drive.unit == 0)
        .unwrap();
    drive.media = Some(assembly::FloppyMediaList {
        entries: vec![assembly::FloppyMedium {
            image: assembly::ImageSource::File(path.to_owned()),
            expected_sha256: None,
            write: assembly::WriteMode::ReadOnly,
            name: "levels-1200.img".into(),
        }],
        empty: false,
        inserted: 0,
    });
}

fn level_presenter(config: &assembly::MachineConfig) -> Presenter {
    let assembled = assembly::assemble_with_observation(
        config,
        &ObservationPolicy {
            routes: vec![ObservationRequest {
                device: DeviceId::Vga,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..ObservationPolicy::default()
        },
    )
    .unwrap();
    presenter_from_assembled(config, assembled, &mut io::sink()).unwrap()
}

/// The shipped open AT games machine file with its firmware and pinned media in
/// a temporary tree, a blank C:, and the level boot sector as A:'s only disk.
#[test]
fn open_games_machine_file_plays_cards_and_speaker_at_host_mix_levels() {
    let fixtures = [
        (
            "vendor/unemu-at-bios/bios-unemu-at.bin",
            "vendor/unemu-at-bios/fetch.sh",
        ),
        ("vendor/vgabios/vgabios-std.bin", "vendor/vgabios/fetch.sh"),
        (
            "vendor/xtide-universal-bios/ide_xtp-r638-ata1f0-8k.bin",
            "vendor/xtide-universal-bios/fetch.sh",
        ),
        // The shipped file pins A:'s catalogue; the test then replaces it.
        ("vendor/freedos/freedos-1200.img", "vendor/freedos/fetch.sh"),
        (
            "vendor/freedos/freedos-tools-1200.img",
            "vendor/freedos/fetch.sh",
        ),
    ];
    let mut found = Vec::new();
    for (relative, fetch) in fixtures {
        let Some(path) = open_fixture(relative, fetch) else {
            return;
        };
        found.push((relative, path));
    }
    let tree = assembly::test_support::TempDir::new("window-open-levels");
    let write = |relative: &str, bytes: &[u8]| {
        let path = tree.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    };
    for (relative, path) in &found {
        write(relative, &std::fs::read(path).unwrap());
    }
    write(
        "machines/at-games-open.toml",
        &std::fs::read(default_fixture("machines/at-games-open.toml")).unwrap(),
    );
    write("machines/local/transfer-1200.img", &vec![0; 1_228_800]);
    write("machines/local/games.img", &vec![0; 615 * 4 * 17 * 512]);
    let mut config = machine_file::load(tree.path().join("machines/at-games-open.toml")).unwrap();
    insert_level_disk(&mut config, &tree.path().join("levels-1200.img"));
    assert_open_profile_levels("machine file", phase_peaks(level_presenter(&config)));
}

/// The open-at-games template image as the library starts it: a fresh
/// workspace session with its verified bases, and the level boot sector put in
/// its empty A: drive.
#[test]
fn open_games_template_image_plays_cards_and_speaker_at_host_mix_levels() {
    let vendor = default_fixture("vendor");
    for source in pc_image::template::FIRMWARE {
        for relative in [source.rom, source.source_archive] {
            let fetch = format!("vendor/{}/fetch.sh", relative.split('/').next().unwrap());
            if open_fixture(&format!("vendor/{relative}"), &fetch).is_none() {
                return;
            }
        }
    }
    let temp = assembly::test_support::TempDir::new("window-template-levels");
    let store = image_store::Store::open(temp.path().join("store")).unwrap();
    pc_image::template::build(&store, &vendor, &|_, _, _| Ok(()), &mut |_| {}).unwrap();
    let image = store.tag_descriptor("open-at-games:latest").unwrap();
    let owner = store
        .workspace(&crate::library_worker::regular_name(&image.digest))
        .unwrap();
    let mut prepared = pc_image::workspace::prepare_pinned(&owner, &image, true).unwrap();
    insert_level_disk(&mut prepared.machine, &temp.path().join("levels-1200.img"));
    let assembled = assembly::assemble_with_verified_bases(
        &prepared.machine,
        &ObservationPolicy {
            routes: vec![ObservationRequest {
                device: DeviceId::Vga,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..ObservationPolicy::default()
        },
        assembly::AssemblyOptions::default(),
        &prepared.bases,
    )
    .unwrap();
    let presenter =
        presenter_from_assembled(&prepared.machine, assembled, &mut io::sink()).unwrap();
    assert_open_profile_levels("template image", phase_peaks(presenter));
}
