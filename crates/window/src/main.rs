// SPDX-License-Identifier: AGPL-3.0-or-later
//! Desktop shell and the window of a running PC (design/spec.md §9.4).
//!
//! The paced session is `machine_view::Session`, which drives the machine
//! through the front-end contract (`backlog/docs/design/core-contract.md`
//! §5): pacing against the host clock, host audio, pointer capture, the media
//! and mute chords. The machine is a `unemu_pc_core::PcMachine` wrapping the
//! PC the provider assembled; it encodes keys and pointer motion for the 8042
//! and the mouse, rasterises the text modes and samples the drives' activity
//! after each run chunk for the status-bar lights. `Presenter` adds the PC
//! catalogue model the media dialogs append to. The Media menu names both
//! floppy media. See `backlog/docs/design/observation.md` § Window pacing and
//! § Host input.

use assembly::{AssembledMachine, DeviceId, ObservationPolicy, ObservationRequest, OutputKind};
use machine_view::Session;
#[cfg(test)]
use std::path::Path;
use std::{error::Error, ffi::OsStr, io, path::PathBuf, time::Duration};
use ui_kit::{EGUI_FONT_NOTICE, SYMBOL_FONT_NOTICE};
use unemu_core_api::DisplayId;
use unemu_pc_core::{PcMachine, FONT_NOTICE};
#[cfg(test)]
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
};

/// The session's pacing, and the AT's tick rate for the tests that pace by it.
mod pacing {
    #[cfg(test)]
    pub(crate) use machine_view::pacing::next_frame;
    pub(crate) use machine_view::pacing::{wake, Speed};

    /// The AT's timer input, 14.31818 MHz divided by twelve (IBM Personal
    /// Computer AT Technical Reference, System Board, "System Timers"): the
    /// tick rate every PC describes.
    #[cfg(test)]
    pub(crate) const PIT: unemu_core_api::Rate = unemu_core_api::Rate::new(105_000_000, 88);

    #[cfg(test)]
    pub(crate) fn frame_target(
        elapsed: std::time::Duration,
        clock: u64,
        dropped: u128,
        ratio: u32,
    ) -> (u64, u128) {
        machine_view::pacing::frame_target(elapsed, clock, dropped, ratio, PIT)
    }

    /// Elapsed host time as ticks of the AT's timer input.
    #[cfg(test)]
    pub(crate) fn virtual_ticks(elapsed: std::time::Duration) -> u128 {
        machine_view::pacing::virtual_ticks(elapsed, PIT)
    }
}
pub(crate) use machine_view::system_keys;
// The PC media and library models, under the paths the desktop names them by.
use pc_library::{firmware_choices, library_worker, new_machine, pc_desktop};
use pc_media::{created_media, drive_chips, kept_catalogue};
// The PC media views, under the path the shell names them by.
use media_ui::catalogue_menu;
// The PC dialogs, under the paths the library frame and the shell name them by.
use pc_ui::{new_machine_view, settings_sheet};

mod app_menu;
#[cfg(test)]
mod host_audio;
mod image_settings;
#[cfg(test)]
mod key_table_tests;
mod layers_dialog;
#[cfg(test)]
mod library_test_support;
mod library_view;
#[cfg(target_os = "macos")]
mod macos_menu;
mod media_dialog;
mod media_picker;
#[cfg(test)]
mod mouse;
mod preferences;
mod preferences_sheet;
mod registries_model;
mod registry_transfer;
mod registry_transfer_view;
mod remote_browse;
mod remote_pull;
mod save_image;
mod settings_model;
mod shell;
#[cfg(test)]
mod shell_icons;
mod shell_model;
#[cfg(test)]
mod sound_levels_tests;

#[cfg(test)]
use pacing::{wake, Speed};

/// The set-1 codes of a host key: the session's mapping to a keyboard usage,
/// then the PC core's encoder, as the guest receives them.
#[cfg(test)]
fn set1_scancodes(key: KeyCode, state: ElementState) -> Option<Vec<u8>> {
    unemu_pc_core::set1_scancodes(
        machine_view::keys::key_usage(key)?,
        if state == ElementState::Pressed {
            unemu_core_api::KeyState::Pressed
        } else {
            unemu_core_api::KeyState::Released
        },
    )
}
#[cfg(test)]
const BIOS_FIXTURE: &str = "vendor/kiselev-bios/bios-xi8088.bin";
#[cfg(test)]
const FLOPPY_FIXTURE: &str = "vendor/freedos/freedos.img";
#[cfg(test)]
const BIOS_FETCH: &str = "bash vendor/kiselev-bios/fetch.sh";
#[cfg(test)]
const FLOPPY_FETCH: &str = "bash vendor/freedos/fetch.sh";

#[cfg(test)]
fn default_fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[cfg(test)]
fn read_fixture(path: &Path, fetch: &str) -> io::Result<Vec<u8>> {
    std::fs::read(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "cannot read {}: {error}; fetch fixtures with `{fetch}` from the workspace root",
                path.display()
            ),
        )
    })
}

/// Waiting on background threads in tests: the library's hang guard.
#[cfg(test)]
use pc_library::test_wait;

/// The desktop's session of one PC: the contract session, and the PC
/// catalogue model the media dialogs edit beside it.
///
/// The session drives the machine through the contract alone. The media
/// dialogs, the catalogue menus and the drive chips still read and append the
/// PC's floppy catalogue through [`PcMachine::assembled`], with the machine
/// config and the drives' catalogues kept here.
struct Presenter {
    session: Session<PcMachine>,
    /// The card the session shows; the tests check which one a machine file
    /// selects.
    #[cfg_attr(not(test), allow(dead_code))]
    display: DeviceId,
    media_slots: [Option<media_dialog::FloppySlot>; 2],
    media_config: assembly::MachineConfig,
    /// Status-bar drive lights; host UI time only, never read by the machine.
    drive_lights: drive_chips::DriveLights,
}

impl std::ops::Deref for Presenter {
    type Target = Session<PcMachine>;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl std::ops::DerefMut for Presenter {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}

impl Presenter {
    #[cfg(test)]
    fn new(
        config: &assembly::MachineConfig,
        options: assembly::AssemblyOptions,
        warnings: &mut impl io::Write,
    ) -> Result<Self, Box<dyn Error>> {
        let assembled = assembly::assemble_with_observation_and_options(
            config,
            &ObservationPolicy {
                routes: vec![ObservationRequest {
                    device: DeviceId::Mda,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..ObservationPolicy::default()
            },
            options,
        )?;
        Self::from_assembled(config, assembled, DeviceId::Mda, warnings)
    }

    /// Wrap a machine assembled with `display` as its one observed card.
    fn from_assembled(
        config: &assembly::MachineConfig,
        assembled: AssembledMachine,
        display: DeviceId,
        warnings: &mut impl io::Write,
    ) -> Result<Self, Box<dyn Error>> {
        let mut media_slots = media_dialog::FloppySlot::from_config(config);
        for (unit, slot) in media_slots.iter_mut().enumerate() {
            if let (Some(slot), Some(names)) = (
                slot,
                assembled.floppy_names.get(&(DeviceId::Fdc, unit as u8)),
            ) {
                for (entry, name) in slot.entries.iter_mut().zip(names) {
                    entry.medium.name = name.clone();
                }
            }
        }
        let machine = PcMachine::from_assembled(config, assembled)?;
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut session = Session::new(Box::new(machine), DisplayId(0), warnings)?;
        // The shell tests start real machines, whose BIOS plays its POST
        // melody: on a developer's machine that came out of the speakers at
        // random moments during a test run. Tests behave as a host without
        // an output device does.
        #[cfg(test)]
        session
            .audio
            .refuse_device("host audio output is not opened in tests");
        Ok(Self {
            session,
            display,
            media_slots,
            media_config: config.clone(),
            drive_lights: drive_chips::DriveLights::default(),
        })
    }

    fn assembled(&mut self) -> &mut AssembledMachine {
        self.session.machine_mut().assembled()
    }

    fn assembled_ref(&self) -> &AssembledMachine {
        self.session.machine().assembled_ref()
    }

    /// The machine, its config and the drives' catalogues at once, for the
    /// media dialogs that append to all three.
    fn media_parts(
        &mut self,
    ) -> (
        &mut AssembledMachine,
        &mut assembly::MachineConfig,
        &mut [Option<media_dialog::FloppySlot>; 2],
    ) {
        (
            self.session.machine_mut().assembled(),
            &mut self.media_config,
            &mut self.media_slots,
        )
    }

    /// The PC's keyboard FIFO, for the tests that watch what waits for the 8042.
    #[cfg(test)]
    fn keyboard(&self) -> &unemu_pc_core::HostKeyboard {
        self.session.machine().keyboard()
    }

    /// The catalogue entry floppy drive `unit` holds now.
    fn inserted_floppy(&mut self, unit: usize) -> Option<usize> {
        self.session.machine_mut().inserted_floppy(unit)
    }

    /// Hand this frame's busy drives to the lights at host time `now`.
    fn observe_drive_lights(&mut self, now: Duration) {
        let mut busy = [false; drive_chips::DRIVES];
        for slot in self.session.take_slot_activity() {
            if let Some(seen) = busy.get_mut(usize::from(slot.0)) {
                *seen = true;
            }
        }
        self.drive_lights.observe(busy, now);
    }

    /// Every drive with the medium it holds now.
    fn drive_views(&mut self, origin: &drive_chips::Origin) -> Vec<drive_chips::DriveView> {
        let inserted = std::array::from_fn(|unit| {
            let index = self.inserted_floppy(unit)?;
            Some((
                index,
                self.assembled().floppy_name(DeviceId::Fdc, unit as u8),
            ))
        });
        drive_chips::drive_views(&self.media_config, &self.media_slots, &inserted, origin)
    }

    /// Seal the machine's journals and write its warnings once.
    fn finish_persistence(&mut self, warnings: &mut impl io::Write) -> Result<(), Box<dyn Error>> {
        self.session.finish(warnings)
    }

    fn write_reset_summary(&self, diagnostics: &mut impl io::Write) {
        if let Some(summary) = self.session.machine().reset_summary() {
            let _ = writeln!(diagnostics, "{summary}");
        }
    }
}

fn informational_output(arg: &OsStr) -> Option<String> {
    let opl3 = devices::OPL3_NOTICE;
    let shown = shown_path;
    if arg == "--help" {
        Some(format!(
            "usage: unemupc [--machine FILE] [--display CARD] [--rom-dir DIR]... [--allow-unlisted-firmware]\n\ndefault selection: {} (opens the machine library; --machine starts immediately)\nmachine files: {} (set {MACHINES_DIR_VAR} to use another directory)\n\n{FONT_NOTICE}\n\n{EGUI_FONT_NOTICE}\n\n{SYMBOL_FONT_NOTICE}\n\n{opl3}", shown(default_machine()), shown(machines_dir())
        ))
    } else if arg == "--version" || arg == "--about" {
        Some(format!(
            "unemupc {}\n{}\n\n{FONT_NOTICE}\n\n{EGUI_FONT_NOTICE}\n\n{SYMBOL_FONT_NOTICE}\n\n{opl3}",
            env!("CARGO_PKG_VERSION"),
            platforms::licence_notice(),
        ))
    } else {
        None
    }
}

/// A default path for `--help`, or why there is none.
fn shown_path(path: Option<PathBuf>) -> String {
    path.map_or_else(
        || "none (the host has no data directory)".into(),
        |path| path.display().to_string(),
    )
}

/// Names the directory of machine files the library lists. A release binary
/// knows nothing of the tree it was built from; see [`machines_dir`].
const MACHINES_DIR_VAR: &str = "UNEMUPC_MACHINES_DIR";

/// The directory of machine files: [`MACHINES_DIR_VAR`] when set; else the
/// source tree's `machines` when cargo started the program (`cargo run` names
/// the crate's directory in `CARGO_MANIFEST_DIR` at run time); else
/// `unemupc/machines` in the platform data directory. None when the host has
/// no data directory either.
fn machines_dir() -> Option<PathBuf> {
    machines_dir_from(
        std::env::var_os(MACHINES_DIR_VAR),
        std::env::var_os("CARGO_MANIFEST_DIR"),
        image_store::location::data_directory().ok(),
    )
}

/// [`machines_dir`] for a given environment. Every workspace crate sits two
/// levels below the root, so `crate_dir/../..` is the source tree.
fn machines_dir_from(
    switch: Option<std::ffi::OsString>,
    crate_dir: Option<std::ffi::OsString>,
    data: Option<PathBuf>,
) -> Option<PathBuf> {
    switch
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate_dir
                .filter(|path| !path.is_empty())
                .map(|crate_dir| PathBuf::from(crate_dir).join("../../machines"))
        })
        .or_else(|| data.map(|data| data.join("unemupc").join("machines")))
}

fn default_machine() -> Option<PathBuf> {
    machines_dir().map(|directory| directory.join("at-freedos.toml"))
}

#[derive(Debug)]
struct Args {
    machine: PathBuf,
    rom_dirs: Vec<PathBuf>,
    display: Option<String>,
    options: assembly::AssemblyOptions,
    /// Print speaker and CPU-reset events to stderr; see [`SPEAKER_TRACE_VAR`].
    speaker_trace: bool,
}

/// Setting this to `1` makes the window print one stderr line per speaker
/// start, tone change and CPU reset, with the guest CS:IP. It is read once
/// at startup; any other value or its absence leaves the trace off.
const SPEAKER_TRACE_VAR: &str = "UNEMUPC_TRACE_SPEAKER";

fn speaker_trace_requested(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args, &'static str> {
    let mut result = Args {
        // Without a data directory there is no default; the library then
        // selects nothing, and starting needs --machine.
        machine: default_machine().unwrap_or_default(),
        rom_dirs: Vec::new(),
        display: None,
        options: assembly::AssemblyOptions::default(),
        speaker_trace: false,
    };
    let mut machine_seen = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--allow-unlisted-firmware") => result.options.allow_unlisted_firmware = true,
            Some("--rom-dir") => {
                let path = args.next().filter(|path| !path.is_empty() && !path.as_encoded_bytes().starts_with(b"--")).ok_or("missing ROM directory")?;
                result.rom_dirs.push(path.into());
            }
            Some("--machine" | "--display") => {
                let value = args.next().filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("missing option value")?;
                if arg == "--machine" && !machine_seen {
                    result.machine = value.into();
                    machine_seen = true;
                } else if arg == "--display" && result.display.is_none() {
                    result.display = Some(value.into_string().map_err(|_| "invalid card id")?);
                } else {
                    return Err("duplicate option");
                }
            }
            _ => return Err("expected --machine FILE, --display CARD or --allow-unlisted-firmware; positional paths are no longer supported"),
        }
    }
    Ok(result)
}

fn selected_display(
    loaded: &machine_file::LoadedMachine,
    requested: Option<&str>,
) -> Result<DeviceId, Box<dyn Error>> {
    loaded
        .cards
        .iter()
        .find(|(id, device)| {
            requested.is_none_or(|requested| requested == id)
                && matches!(device, DeviceId::Mda | DeviceId::Vga)
        })
        .map(|(_, device)| *device)
        .ok_or_else(|| {
            match requested {
                Some(id) => format!("display card {id:?} is not configured"),
                None => "machine has no display card".into(),
            }
            .into()
        })
}

#[cfg(test)]
fn load_machine(
    path: &Path,
    default: &Path,
    options: assembly::AssemblyOptions,
) -> Result<machine_file::LoadedMachine, Box<dyn Error>> {
    load_machine_with_rom_dirs(path, default, options, &[])
}

#[cfg(test)]
fn load_machine_with_rom_dirs(
    path: &Path,
    default: &Path,
    options: assembly::AssemblyOptions,
    roots: &[PathBuf],
) -> Result<machine_file::LoadedMachine, Box<dyn Error>> {
    machine_file::load_for_assembly_with_rom_dirs(path, options, roots).map_err(|error| {
        if path == default && !path.exists() {
            format!("{error}; use --machine FILE to select a machine description")
        } else {
            error.to_string()
        }
        .into()
    })
}

#[cfg(test)]
fn setup(args: &Args, warnings: &mut impl io::Write) -> Result<Presenter, Box<dyn Error>> {
    setup_with_rom_dirs(args, &args.rom_dirs, warnings)
}

#[cfg(test)]
fn setup_with_rom_dirs(
    args: &Args,
    roots: &[PathBuf],
    warnings: &mut impl io::Write,
) -> Result<Presenter, Box<dyn Error>> {
    let loaded = load_machine_with_rom_dirs(
        &args.machine,
        &default_machine().unwrap_or_default(),
        args.options,
        roots,
    )?;
    setup_loaded(args, &loaded, warnings)
}

fn setup_loaded(
    args: &Args,
    loaded: &machine_file::LoadedMachine,
    warnings: &mut impl io::Write,
) -> Result<Presenter, Box<dyn Error>> {
    let display = selected_display(loaded, args.display.as_deref())?;
    let assembled = loaded.assemble(&ObservationPolicy {
        routes: vec![ObservationRequest {
            device: display,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        speaker_trace: args.speaker_trace,
        ..ObservationPolicy::default()
    })?;
    Presenter::from_assembled(&loaded.config, assembled, display, warnings)
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let [arg] = args.as_slice() {
        if let Some(output) = informational_output(arg) {
            println!("{output}");
            return Ok(());
        }
    }
    let mut args = parse_args(args)?;
    args.speaker_trace = speaker_trace_requested(std::env::var_os(SPEAKER_TRACE_VAR).as_deref());
    let autostart = std::env::args_os().skip(1).any(|arg| arg == "--machine");
    shell::run(args, autostart)?;
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assembly::assemble_with_observation;

    /// An idle screen is copied out of the machine once, not on every paint.
    #[test]
    fn an_unchanged_screen_is_copied_once_across_paints() {
        let mut rom = vec![0u8; 0x8000];
        rom[..2].copy_from_slice(&[0xEB, 0xFE]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        for frame in 1..=30 {
            presenter
                .advance_fully(Duration::from_nanos(frame * 16_666_667))
                .unwrap();
        }
        assert!(presenter.running);
        assert_eq!(presenter.picture_copies, 1);
    }

    /// A wrong pin of a core's contract fails here, not in a user's hands.
    #[test]
    fn every_compiled_in_core_implements_the_contract_the_desktop_drives() {
        let cores: [&dyn unemu_core_api::Core; 1] = [&unemu_pc_core::PcCore];
        for core in cores {
            let description = core.description();
            assert!(
                unemu_core_api::CONTRACT.drives(description.implements),
                "core {} implements contract {}; this program needs {}",
                description.id,
                description.implements,
                unemu_core_api::CONTRACT
            );
        }
    }

    #[test]
    fn speaker_trace_is_off_unless_the_switch_is_exactly_one() {
        assert!(!speaker_trace_requested(None));
        for value in ["", "0", "true", "yes", " 1", "11"] {
            assert!(
                !speaker_trace_requested(Some(OsStr::new(value))),
                "{value:?}"
            );
        }
        assert!(speaker_trace_requested(Some(OsStr::new("1"))));
        assert!(!parse_args([]).unwrap().speaker_trace);
    }

    /// IBM Personal Computer AT Technical Reference, System Board, Speaker:
    /// the guest sets counter 2 to 1193 (about 1 kHz) and port 61h bits 0-1.
    #[test]
    fn presenter_prints_speaker_trace_lines_only_when_enabled() {
        let program = [
            0xFA, 0xB0, 0xB6, 0xE6, 0x43, 0xB0, 0xA9, 0xE6, 0x42, 0xB0, 0x04, 0xE6, 0x42, 0xB0,
            0x03, 0xE6, 0x61, 0xEB, 0xFE,
        ];
        let mut rom = vec![0u8; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        for trace in [false, true] {
            let assembled = assembly::assemble_with_observation_and_options(
                &config,
                &ObservationPolicy {
                    routes: vec![ObservationRequest {
                        device: DeviceId::Mda,
                        output: OutputKind::TextScanout,
                        byte_limit: 0,
                    }],
                    speaker_trace: trace,
                    ..ObservationPolicy::default()
                },
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
            )
            .unwrap();
            let mut warnings = Vec::new();
            let mut presenter =
                Presenter::from_assembled(&config, assembled, DeviceId::Mda, &mut warnings)
                    .unwrap();
            presenter
                .advance_fully_with_warnings(Duration::from_millis(20), &mut warnings)
                .unwrap();
            let text = String::from_utf8(warnings).unwrap();
            if trace {
                let lines: Vec<_> = text.lines().collect();
                assert_eq!(lines.len(), 2, "{text}");
                assert!(lines[0].starts_with("speaker trace on"), "{text}");
                assert!(lines[1].starts_with("speaker on    t="), "{text}");
                assert!(
                    lines[1].ends_with("cs:ip=F800:0011 mode 3 divisor 1193 (1000.1 Hz)"),
                    "{text}"
                );
            } else {
                assert_eq!(text, "");
            }
        }
    }

    fn test_presenter(config: &assembly::MachineConfig) -> Result<Presenter, Box<dyn Error>> {
        Presenter::new(
            config,
            assembly::AssemblyOptions::default(),
            &mut io::sink(),
        )
    }

    #[test]
    fn floppy_media_host_keys_queue_at_boundaries_suppress_repeat_and_select_b_with_shift() {
        use ElementState::{Pressed, Released};
        let mut rom = vec![0xf4; 32768];
        rom[..3].copy_from_slice(&[0xfa, 0xeb, 0xfe]);
        rom[0x7ff0..0x7ff5].copy_from_slice(&[0xea, 0, 0, 0, 0xf8]);
        let mut config = assembly::bios(rom, Some(vec![0; 1_474_560]));
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: ["one.img", "two.img"]
                .into_iter()
                .map(|name| assembly::FloppyMedium {
                    image: assembly::ImageSource::Inline(vec![0; 1_474_560]),
                    expected_sha256: None,
                    write: assembly::WriteMode::ReadOnly,
                    name: name.into(),
                })
                .collect(),
            inserted: 0,
        });
        let mut b = config.drives[0].clone();
        b.unit = 1;
        config.drives.push(b);
        let mut presenter = Presenter::new(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap();
        for key in [
            KeyCode::ControlLeft,
            KeyCode::AltLeft,
            KeyCode::KeyF,
            KeyCode::KeyF,
        ] {
            presenter.key_event(key, Pressed);
        }
        presenter.key_event(KeyCode::KeyF, Released);
        assert_eq!(
            presenter.media_events,
            vec![(unemu_core_api::SlotId(0), unemu_core_api::MediaAction::Next)]
        );
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "one.img"
        );
        presenter.advance_fully(Duration::ZERO).unwrap();
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "one.img"
        );
        advance_frame(&mut presenter);
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "two.img"
        );
        presenter.key_event(KeyCode::ShiftLeft, Pressed);
        presenter.key_event(KeyCode::KeyE, Pressed);
        presenter.key_event(KeyCode::ControlLeft, Released);
        presenter.key_event(KeyCode::KeyE, Released);
        advance_frame(&mut presenter);
        assert_eq!(presenter.assembled().floppy_name(DeviceId::Fdc, 1), "empty");
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "two.img"
        );
        assert!(presenter
            .machine()
            .input_log
            .iter()
            .flat_map(|(_, codes, _)| codes)
            .all(|code| ![0x21, 0xa1, 0x12, 0x92].contains(code)));
        presenter.assembled().finish_persistence().unwrap();
    }

    /// A machine whose firmware counts each start in RAM byte 0500h, then
    /// halts with interrupts off, and whose A: holds `one.img` of two disks.
    pub(super) fn counting_halt_presenter() -> Presenter {
        #[rustfmt::skip]
        let program = [
            0x31, 0xC0, 0x8E, 0xD8, // xor ax,ax; mov ds,ax
            0xFE, 0x06, 0x00, 0x05, // inc byte [0500h]
            0xFA, 0xF4,             // cli; hlt
        ];
        let mut rom = vec![0xF4; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
        let mut config = assembly::bios(rom, Some(vec![0; 1_474_560]));
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: ["one.img", "two.img"]
                .into_iter()
                .map(|name| assembly::FloppyMedium {
                    image: assembly::ImageSource::Inline(vec![0; 1_474_560]),
                    expected_sha256: None,
                    write: assembly::WriteMode::ReadOnly,
                    name: name.into(),
                })
                .collect(),
            inserted: 0,
        });
        Presenter::new(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap()
    }

    pub(super) fn starts_counted(presenter: &Presenter) -> u8 {
        match cpu::Cpu::guest_dma_read(presenter.assembled_ref().machine.cpu(), 0x500) {
            cpu::DmaAccess::Complete(count) => count,
            cpu::DmaAccess::Mmio => panic!("0500h is RAM"),
        }
    }

    #[test]
    fn halted_presenter_changes_floppies_and_reset_runs_the_firmware_again() {
        let mut presenter = counting_halt_presenter();
        presenter.advance_fully(Duration::from_millis(20)).unwrap();
        assert!(!presenter.running);
        assert!(presenter.halted);
        assert!(presenter.error.is_none());
        assert_eq!(starts_counted(&presenter), 1);
        // Host media keys still reach the drives of the halted machine.
        assert!(presenter.takes_media());
        for (key, state) in [
            (KeyCode::ControlLeft, ElementState::Pressed),
            (KeyCode::AltLeft, ElementState::Pressed),
            (KeyCode::KeyF, ElementState::Pressed),
            (KeyCode::KeyF, ElementState::Released),
        ] {
            presenter.key_event(key, state);
        }
        assert_eq!(
            presenter.media_events,
            vec![(unemu_core_api::SlotId(0), unemu_core_api::MediaAction::Next)]
        );
        let time = presenter.assembled().machine.virtual_time();
        let mut diagnostics = Vec::new();
        presenter.reset_halted(&mut diagnostics).unwrap();
        // The queued change goes in before the CPU leaves reset; the reset
        // itself takes no virtual time.
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "two.img"
        );
        assert_eq!(presenter.assembled().machine.virtual_time(), time);
        assert!(presenter.running);
        assert!(!presenter.halted);
        assert_eq!(presenter.machine().cpu_resets(), 1);
        assert_eq!(diagnostics, b"cpu reset\n");
        presenter.advance_fully(Duration::from_millis(40)).unwrap();
        // The firmware ran again from the reset vector with RAM kept, and the
        // inserted disk stayed in its drive.
        assert!(presenter.halted);
        assert_eq!(starts_counted(&presenter), 2);
        assert_eq!(
            presenter.assembled().floppy_name(DeviceId::Fdc, 0),
            "two.img"
        );
        presenter.finish_persistence(&mut io::sink()).unwrap();
    }

    #[test]
    fn reset_of_a_running_or_failed_presenter_changes_nothing() {
        let mut presenter = counting_halt_presenter();
        presenter.reset_halted(&mut io::sink()).unwrap();
        assert!(presenter.running);
        assert_eq!(presenter.machine().cpu_resets(), 0);
        presenter.advance_fully(Duration::from_millis(20)).unwrap();
        presenter.error = Some("cpu failure".into());
        presenter.reset_halted(&mut io::sink()).unwrap();
        assert!(!presenter.running);
        assert!(!presenter.takes_media());
        assert_eq!(presenter.machine().cpu_resets(), 0);
        assert_eq!(starts_counted(&presenter), 1);
    }

    /// A machine whose program writes 55h to CMOS register 10h and halts with
    /// interrupts off; CMOS RAM journals to `journal` over the 64 zero bytes
    /// in `base`.
    pub(super) fn cmos_halt_config(base: &Path, journal: &Path) -> assembly::MachineConfig {
        #[rustfmt::skip]
        let program = vec![
            0xFA,                   // cli
            0xB0, 0x10, 0xE6, 0x70, // mov al,10h; out 70h,al
            0xB0, 0x55, 0xE6, 0x71, // mov al,55h; out 71h,al
            0xF4,                   // hlt
        ];
        let mut config = assembly::at_rtc(program, 100, 10);
        for device in &mut config.devices {
            if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                rtc.nvram = assembly::NvramSource::Journal {
                    image: base.to_owned(),
                    expected_sha256: [
                        245, 165, 253, 66, 209, 106, 32, 48, 39, 152, 239, 110, 211, 9, 151, 155,
                        67, 0, 61, 35, 32, 217, 240, 232, 234, 152, 49, 169, 39, 89, 251, 75,
                    ],
                    journal: journal.to_owned(),
                    keep_generations: 10,
                };
            }
        }
        config
    }

    pub(super) fn vga_presenter(config: &assembly::MachineConfig) -> Presenter {
        let assembled = assemble_with_observation(
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
        Presenter::from_assembled(config, assembled, DeviceId::Vga, &mut io::sink()).unwrap()
    }

    #[test]
    fn cmos_written_before_a_halt_is_durable_without_stop() {
        let temp = assembly::test_support::TempDir::new("window-halt-cmos");
        let base = temp.path().join("cmos.bin");
        std::fs::write(&base, [0; 64]).unwrap();
        let config = cmos_halt_config(&base, &temp.path().join("cmos.upfj"));
        let mut presenter = vga_presenter(&config);
        presenter.advance_fully(Duration::from_millis(20)).unwrap();
        assert!(presenter.halted);
        // As after a crash: the presenter goes away without Stop.
        drop(presenter);
        let mut fresh = assembly::assemble(&config).unwrap();
        let bus = fresh.machine.bus_mut();
        bus.port_write(0x70, 1, 0x10).unwrap();
        assert_eq!(bus.port_read(0x71, 1).unwrap() & 0xFF, 0x55);
        fresh.finish_persistence().unwrap();
    }

    pub(super) fn mouse_presenter(enabled: bool) -> Presenter {
        #[rustfmt::skip]
        let program = [
            0xFA, 0xFC, 0x31, 0xC0, 0x8E, 0xD8, 0x8E, 0xC0, 0xBF, 0, 0x20,
            0xB0, 0xA8, 0xE6, 0x64,
            0xE4, 0x64, 0xA8, 1, 0x74, 0xFA, 0xE4, 0x60, 0xAA, 0xEB, 0xF5,
        ];
        let mut rom = vec![0xF4; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
        let mut config = assembly::bios(rom, Some(vec![0; 1_474_560]));
        for device in &mut config.devices {
            if let assembly::DeviceKind::Keyboard(controller) = &mut device.kind {
                controller.mouse = enabled;
            }
        }
        Presenter::new(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap()
    }

    fn assert_stream_click_survives(previous_unread: bool) {
        use ElementState::{Pressed, Released};
        let mut presenter = mouse_presenter(true);
        let bus = presenter.assembled().machine.bus_mut();
        bus.port_write(0x64, 1, 0xD4).unwrap();
        bus.port_write(0x60, 1, 0xF4).unwrap();
        advance_frame(&mut presenter);
        presenter.mouse.pointer_inside(true);
        presenter.mouse_button(MouseButton::Left, Pressed);
        presenter.mouse_button(MouseButton::Left, Released);
        presenter.mouse_motion(5.0, -7.0);
        if previous_unread {
            presenter.machine_mut().deliver_pointer_for_test();
            let time = presenter.assembled().machine.virtual_time();
            presenter
                .assembled()
                .machine
                .run_bounded(Some(time + 1))
                .unwrap();
        }
        presenter.mouse_button(MouseButton::Left, Pressed);
        presenter.mouse_button(MouseButton::Left, Released);
        presenter.machine_mut().deliver_pointer_for_test();
        for _ in 0..3 {
            advance_frame(&mut presenter);
        }
        let mut bytes = [0; 13];
        presenter
            .assembled()
            .machine
            .cpu()
            .read_mem(0x2000, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0xFA, 8, 5, 7, 9, 0, 0, 8, 0, 0, 0, 0, 0]);
        assert!(presenter
            .machine_mut()
            .deliver_pointer_for_test()
            .is_empty());
        assert!(presenter.window.is_none());
    }

    #[test]
    fn mouse_grab_failure_swallows_click_and_release_then_next_click_can_retry() {
        use ElementState::{Pressed, Released};
        let mut presenter = mouse_presenter(true);
        presenter.mouse.pointer_inside(true);
        presenter.mouse_button_with_grab(MouseButton::Left, Pressed, || {
            Err(winit::error::ExternalError::Ignored)
        });
        assert!(!presenter.mouse.captured());
        presenter.mouse_motion(4.0, 5.0);
        presenter.mouse_button_with_grab(MouseButton::Left, Released, || {
            panic!("the capture-click release must not attempt another grab")
        });
        assert!(presenter
            .machine_mut()
            .deliver_pointer_for_test()
            .is_empty());
        let mut retried = false;
        presenter.mouse_button_with_grab(MouseButton::Left, Pressed, || {
            retried = true;
            Ok(())
        });
        assert!(retried);
        assert!(presenter.mouse.captured());
        presenter.mouse_button(MouseButton::Left, Released);
        assert!(presenter
            .machine_mut()
            .deliver_pointer_for_test()
            .is_empty());
        presenter.mouse_button(MouseButton::Left, Pressed);
        presenter.mouse_button(MouseButton::Left, Released);
        let delivered = presenter.machine_mut().deliver_pointer_for_test();
        assert_eq!(
            delivered
                .iter()
                .map(|input| input.buttons)
                .collect::<Vec<_>>(),
            [1, 0]
        );
        assert!(presenter.window.is_none());
    }

    #[test]
    fn mouse_readiness_query_preserves_controller_state_and_checks_its_own_output() {
        let mut presenter = mouse_presenter(true);
        assert!(presenter.assembled().mouse_input_ready());
        let bus = presenter.assembled().machine.bus_mut();
        bus.port_write(0x64, 1, 0xD4).unwrap();
        bus.port_write(0x60, 1, 0xF4).unwrap();
        advance_frame(&mut presenter);
        let index = presenter.assembled().device_index[&DeviceId::Keyboard];
        assert!(assembly::inject_mouse(
            &mut presenter.assembled().machine,
            index,
            1,
            2,
            0
        ));
        let before = presenter
            .assembled()
            .machine
            .bus()
            .device(index)
            .unwrap()
            .save();
        assert!(!presenter.assembled().mouse_input_ready());
        assert_eq!(
            presenter
                .assembled()
                .machine
                .bus()
                .device(index)
                .unwrap()
                .save(),
            before
        );
        advance_frame(&mut presenter);
        assert!(presenter.assembled().mouse_input_ready());
        assert!(assembly::try_inject_scancode(
            &mut presenter.assembled().machine,
            index,
            0x1E
        ));
        assert!(presenter.assembled().mouse_input_ready());
        assert_eq!(
            assembly::keyboard_leds_if_ready(&mut presenter.assembled().machine, index),
            None
        );
        let mut absent = mouse_presenter(false);
        assert!(!absent.assembled().mouse_input_ready());
        presenter
            .assembled()
            .device_index
            .remove(&DeviceId::Keyboard);
        assert!(!presenter.assembled().mouse_input_ready());
    }

    #[test]
    fn mouse_stream_click_after_motion_in_one_boundary_reaches_guest_down_then_up() {
        assert_stream_click_survives(false);
    }

    #[test]
    fn mouse_stream_click_with_previous_packet_unread_reaches_guest_down_then_up() {
        assert_stream_click_survives(true);
    }

    #[test]
    fn mouse_presenter_injects_only_at_run_boundaries_and_guest_reads_coalesced_counts() {
        use unemu_pc_core::MouseInput;
        use ElementState::{Pressed, Released};
        let mut presenter = mouse_presenter(true);
        advance_frame(&mut presenter);
        let index = presenter.assembled().device_index[&DeviceId::Keyboard];
        let before = presenter
            .assembled()
            .machine
            .bus()
            .device(index)
            .unwrap()
            .save();
        let time = presenter.assembled().machine.virtual_time();
        presenter.mouse.pointer_inside(true);
        presenter.mouse_button(MouseButton::Left, Pressed);
        presenter.mouse_button(MouseButton::Left, Released);
        presenter.mouse_motion(1.5, 2.0);
        presenter.mouse_motion(2.5, -3.0);
        presenter.mouse_motion(4.0, 5.0);
        presenter.mouse_button(MouseButton::Right, Pressed);
        presenter.mouse_motion(5.0, -2.0);
        assert_eq!(
            presenter
                .assembled()
                .machine
                .bus()
                .device(index)
                .unwrap()
                .save(),
            before
        );
        presenter.advance_fully(Duration::ZERO).unwrap();
        assert!(presenter.machine().mouse_log.is_empty());
        advance_frame(&mut presenter);
        assert_eq!(
            presenter.machine().mouse_log,
            [(
                time,
                vec![
                    MouseInput {
                        dx: 8,
                        dy: -4,
                        buttons: 0
                    },
                    MouseInput {
                        dx: 5,
                        dy: 2,
                        buttons: 2
                    },
                ]
            )]
        );
        let bus = presenter.assembled().machine.bus_mut();
        bus.port_write(0x64, 1, 0xD4).unwrap();
        bus.port_write(0x60, 1, 0xEB).unwrap();
        advance_frame(&mut presenter);
        let mut bytes = [0; 7];
        presenter
            .assembled()
            .machine
            .cpu()
            .read_mem(0x2000, &mut bytes)
            .unwrap();
        assert_eq!(bytes, [0xFA, 0x2A, 13, 254, 0, 0, 0]);
        assert_eq!(presenter.machine().mouse_log.len(), 1);
        assert!(presenter.window.is_none());
    }

    #[test]
    fn mouse_presenter_release_keeps_keyboard_balanced_and_ignores_stopped_input() {
        use ElementState::{Pressed, Released};
        for focus_loss in [false, true] {
            let mut presenter = mouse_presenter(true);
            presenter.mouse.pointer_inside(true);
            presenter.mouse_button(MouseButton::Left, Pressed);
            presenter.mouse_button(MouseButton::Left, Released);
            presenter.mouse_button(MouseButton::Right, Pressed);
            presenter.key_event(KeyCode::ControlLeft, Pressed);
            presenter.key_event(KeyCode::AltLeft, Pressed);
            if focus_loss {
                presenter.focus_changed(false);
            } else {
                presenter.key_event(KeyCode::KeyG, Pressed);
                presenter.key_event(KeyCode::KeyG, Pressed);
                presenter.key_event(KeyCode::KeyG, Released);
                presenter.key_event(KeyCode::ControlLeft, Released);
                presenter.key_event(KeyCode::AltLeft, Released);
            }
            assert!(!presenter.mouse.captured());
            assert_eq!(presenter.keyboard().pending_events(), 4);
            advance_frame(&mut presenter);
            assert_eq!(
                presenter.machine().mouse_log[0]
                    .1
                    .iter()
                    .map(|input| input.buttons)
                    .collect::<Vec<_>>(),
                [2, 0]
            );
            presenter.running = false;
            presenter.mouse.pointer_inside(true);
            presenter.mouse_button(MouseButton::Left, Pressed);
            presenter.mouse_motion(20.0, 30.0);
            assert!(!presenter.mouse.captured());
            assert!(presenter
                .machine_mut()
                .deliver_pointer_for_test()
                .is_empty());
            assert!(presenter.window.is_none());
        }
    }

    #[test]
    fn mouse_presenter_without_mouse_preserves_keyboard_and_device_state() {
        use ElementState::{Pressed, Released};
        let mut presenter = mouse_presenter(false);
        let index = presenter.assembled().device_index[&DeviceId::Keyboard];
        let before = presenter
            .assembled()
            .machine
            .bus()
            .device(index)
            .unwrap()
            .save();
        presenter.mouse.pointer_inside(true);
        presenter.mouse_button(MouseButton::Left, Pressed);
        presenter.mouse_motion(10.0, 20.0);
        presenter.mouse_button(MouseButton::Left, Released);
        assert!(!presenter.mouse.captured());
        assert!(presenter
            .machine_mut()
            .deliver_pointer_for_test()
            .is_empty());
        assert_eq!(
            presenter
                .assembled()
                .machine
                .bus()
                .device(index)
                .unwrap()
                .save(),
            before
        );
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyG] {
            presenter.key_event(key, Pressed);
            presenter.key_event(key, Released);
        }
        assert_eq!(presenter.keyboard().pending_events(), 6);
        assert!(presenter.window.is_none());
    }

    #[test]
    fn machine_file_setup_equals_the_preset_without_opening_a_window() {
        let required = std::env::var_os("UNEMUPC_REQUIRE_FIXTURES");
        let fixtures = [(BIOS_FIXTURE, BIOS_FETCH), (FLOPPY_FIXTURE, FLOPPY_FETCH)].map(
            |(relative, fetch)| {
                let path = default_fixture(relative);
                fixture_result(
                    read_fixture(&path, fetch),
                    &path,
                    fetch,
                    required.as_deref(),
                )
            },
        );
        let [Some(rom), Some(floppy)] = fixtures else {
            return;
        };
        let machine = default_fixture("machines/at-freedos.toml");
        let loaded =
            machine_file::load_for_assembly(&machine, assembly::AssemblyOptions::default())
                .unwrap();
        assert_eq!(loaded.config, assembly::bios(rom, Some(floppy)));
        let args = parse_args(["--machine".into(), machine.clone().into_os_string()]).unwrap();
        assert_eq!(args.machine, machine);
        let presenter = setup(&args, &mut io::sink()).unwrap();
        assert_eq!(presenter.display, DeviceId::Mda);
        assert!(presenter.window.is_none());
        assert_eq!(presenter.assembled_ref().machine.virtual_time(), 0);
    }

    #[test]
    fn display_flag_selects_card_identity_and_default_follows_file_order() {
        let temp = assembly::test_support::TempDir::new("window-machine-display");
        std::fs::write(temp.path().join("bios.bin"), vec![0xF4; 32768]).unwrap();
        let file = temp.path().join("machine.toml");
        std::fs::write(&file, "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\n[[card]]\nkind='vga-text'\nid='colour'\n[[card]]\nkind='mda'\nid='mono'\n").unwrap();
        let args_for = |display: Option<&str>| {
            let mut args = vec![
                "--machine".into(),
                file.clone().into_os_string(),
                "--allow-unlisted-firmware".into(),
            ];
            if let Some(display) = display {
                args.extend(["--display".into(), display.into()]);
            }
            parse_args(args).unwrap()
        };
        for (name, expected) in [
            (None, DeviceId::Vga),
            (Some("mono"), DeviceId::Mda),
            (Some("colour"), DeviceId::Vga),
        ] {
            let presenter = setup(&args_for(name), &mut io::sink()).unwrap();
            assert_eq!(presenter.display, expected);
            assert_eq!(
                presenter.assembled_ref().observation.routes()[0].device,
                expected
            );
            assert!(presenter.assembled_ref().capture_text(expected).is_ok());
            assert!(presenter.window.is_none());
        }
        assert!(setup(&args_for(Some("missing")), &mut io::sink()).is_err());
        let missing = Args {
            machine: temp.path().join("missing.toml"),
            rom_dirs: Vec::new(),
            display: None,
            options: assembly::AssemblyOptions::default(),
            speaker_trace: false,
        };
        let error = setup(&missing, &mut io::sink()).err().unwrap().to_string();
        assert!(error.contains("missing.toml"), "{error}");
        for args in [
            vec!["--display"],
            vec!["bios.bin", "disk.img"],
            vec!["--machine", "--display"],
            vec!["--display", "mono", "--display", "mono"],
        ] {
            assert!(parse_args(args.into_iter().map(Into::into)).is_err());
        }
    }

    #[test]
    fn missing_default_suggests_machine_and_help_names_the_default_path() {
        let temp = assembly::test_support::TempDir::new("window-missing-default");
        let missing = temp.path().join("missing.toml");
        let error = load_machine(&missing, &missing, assembly::AssemblyOptions::default())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("use --machine FILE"), "{error}");
        assert!(error.contains("missing.toml"));
        let help = informational_output(OsStr::new("--help")).unwrap();
        assert!(help.contains(&format!(
            "default selection: {} (opens the machine library; --machine starts immediately)",
            shown_path(default_machine())
        )));
    }

    #[test]
    fn firmware_flag_reaches_headless_presenter() {
        let args = parse_args(
            ["--machine", "machine.toml", "--allow-unlisted-firmware"]
                .into_iter()
                .map(Into::into),
        )
        .unwrap();
        let options = args.options;
        assert!(options.allow_unlisted_firmware);
        assert_eq!(args.machine, PathBuf::from("machine.toml"));
        assert!(parse_args(["bios.bin".into()]).is_err());
        let config = assembly::bios(vec![0; 32768], Some(vec![0; 1_474_560]));
        assert!(Presenter::new(
            &config,
            assembly::AssemblyOptions::default(),
            &mut io::sink()
        )
        .is_err());
        let mut warnings = Vec::new();
        let presenter = Presenter::new(&config, options, &mut warnings).unwrap();
        assert_eq!(presenter.assembled_ref().unlisted_firmware.len(), 1);
        assert_eq!(String::from_utf8(warnings).unwrap(), "warning: running unlisted firmware (sha256 c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479)\n");
    }

    #[test]
    fn nvram_journal_matches_cli_across_window_frame_slices() {
        let temp = assembly::test_support::TempDir::new("window-nvram");
        let base = temp.path().join("cmos.bin");
        std::fs::write(&base, [0; 64]).unwrap();
        let mut reference = None;
        for lane in ["cli", "window-single", "window-sliced"] {
            let mut program = vec![0xFA, 0xB0, 0x10, 0xE6, 0x70, 0xB0, 1, 0xE6, 0x71];
            // Keep the first value live across frame slices so premature persistence adds a record.
            program.extend([0x90; 64]);
            program.extend([0xB0, 9, 0xE6, 0x71, 0xB0, 0xFE, 0xE6, 0x64]);
            let mut config = assembly::at_rtc(program, 100, 10);
            config.memory.address_space_bytes = 0x100000;
            config.images.push(assembly::LoadImage {
                base: 0xFFFF0,
                source: assembly::ImageSource::Inline(vec![
                    0xB0, 0x10, 0xE6, 0x70, 0xB0, 0x55, 0xE6, 0x71, 0xF4,
                ]),
                kind: assembly::ImageKind::Rom,
                provides: vec![],
            });
            let journal = temp.path().join(format!("{lane}.upfj"));
            for device in &mut config.devices {
                if let assembly::DeviceKind::Rtc(rtc) = &mut device.kind {
                    rtc.nvram = assembly::NvramSource::Journal {
                        image: base.clone(),
                        expected_sha256: [
                            245, 165, 253, 66, 209, 106, 32, 48, 39, 152, 239, 110, 211, 9, 151,
                            155, 67, 0, 61, 35, 32, 217, 240, 232, 234, 152, 49, 169, 39, 89, 251,
                            75,
                        ],
                        journal: journal.clone(),
                        keep_generations: 10,
                    };
                }
            }
            if lane == "cli" {
                let mut assembled = assembly::assemble(&config).unwrap();
                assembled
                    .run_session(&assembly::RunPolicy::natural(Some(200), None))
                    .unwrap();
                assembled.finish_persistence().unwrap();
            } else {
                let assembled = assembly::assemble_with_observation(
                    &config,
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
                let mut presenter =
                    Presenter::from_assembled(&config, assembled, DeviceId::Vga, &mut io::sink())
                        .unwrap();
                let step = if lane == "window-single" { 100 } else { 1 };
                for elapsed in (step..=200).step_by(step) {
                    presenter
                        .advance_fully(Duration::from_micros(elapsed as u64))
                        .unwrap();
                    if !presenter.running {
                        break;
                    }
                }
                assert!(!presenter.running);
                assert!(presenter.error.is_none());
                assert_eq!(presenter.machine().cpu_resets(), 1);
                // A halted guest can still be reset; Stop ends its session.
                assert!(presenter.halted);
                presenter.finish_persistence(&mut io::sink()).unwrap();
            }
            let bytes = std::fs::read(journal).unwrap();
            assert_eq!(
                bytes.len(),
                128 + 5 * 640,
                "two replacements and one session end"
            );
            if let Some(expected) = &reference {
                assert_eq!(&bytes, expected, "{lane}");
            } else {
                reference = Some(bytes);
            }
        }
    }

    /// A guest that starts drive A:'s motor and leaves an IDE read's data
    /// unread, so both drives stay busy at every run boundary.
    pub(crate) fn busy_drives_config() -> assembly::MachineConfig {
        let mut code = vec![0xFA, 0xFC];
        for (port, byte) in [
            (0x3F2u16, 0x1C),
            (0x1F6, 0xE0),
            (0x1F2, 1),
            (0x1F3, 0),
            (0x1F4, 0),
            (0x1F5, 0),
            (0x1F7, 0x20),
        ] {
            code.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, byte, 0xEE]);
        }
        code.extend([0xEB, 0xFE]);
        let demo = assembly::at_boot(code);
        let mut config = assembly::bios(vec![0xF4; 32768], None);
        config.board = assembly::BoardModel::Bare;
        config.cpu = demo.cpu;
        config.images = demo.images;
        for device in &mut config.devices {
            if device.id == DeviceId::Fdc {
                device.ports = assembly::FdcCfg::port_windows(true);
            }
        }
        let ide = assembly::IdeCfg {
            model: assembly::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            assembly::DeviceConfig::new(
                DeviceId::Ide,
                assembly::DeviceKind::Ide(ide),
                ide.port_windows(),
            )
            .with_irq(ide.irq()),
        );
        config.drives.push(assembly::DriveConfig {
            media: None,
            controller: DeviceId::Ide,
            unit: 0,
            kind: assembly::DriveKind::HardDisk {
                geometry: assembly::HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512,
                },
            },
            image: assembly::ImageSource::Inline(vec![0; 8 * 2 * 4 * 512]),
            expected_sha256: None,
            write: assembly::WriteMode::ReadOnly,
        });
        config
    }

    /// A guest that loops over a four-sector IDE read (BSY between blocks) and
    /// a floppy READ DATA through 8237 channel 2 with A:'s motor on, counting
    /// rounds at 7C95h. Assembled from this NASM source (org 7C00h):
    /// `cli; xor ax,ax; mov ds,ax; mov es,ax; out 3F2h<-1Ch`, then per round:
    /// IDE 1F6h<-E0h, 1F2h<-4, LBA 0, 1F7h<-20h; four times wait for DRQ and
    /// `rep insw` 256 words to 1000h; 8237: clear flip-flop, mode 46h, address
    /// 2000h, page 0, count 1FFh, unmask channel 2; send READ DATA
    /// `46 00 00 00 01 02 01 1B FF` on RQM; read seven result bytes on RQM+DIO;
    /// `inc word [7C95h]`.
    fn disk_and_dma_guest_config() -> assembly::MachineConfig {
        #[rustfmt::skip]
        let code = vec![
            0xfa, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0xba, 0xf2, 0x03, 0xb0, 0x1c,
            0xee, 0xba, 0xf6, 0x01, 0xb0, 0xe0, 0xee, 0xba, 0xf2, 0x01, 0xb0, 0x04,
            0xee, 0xba, 0xf3, 0x01, 0x30, 0xc0, 0xee, 0x42, 0xee, 0x42, 0xee, 0xba,
            0xf7, 0x01, 0xb0, 0x20, 0xee, 0xb3, 0x04, 0xba, 0xf7, 0x01, 0xec, 0xa8,
            0x08, 0x74, 0xfb, 0xba, 0xf0, 0x01, 0xbf, 0x00, 0x10, 0xb9, 0x00, 0x01,
            0xf3, 0x6d, 0xfe, 0xcb, 0x75, 0xe9, 0x30, 0xc0, 0xe6, 0x0c, 0xb0, 0x46,
            0xe6, 0x0b, 0x30, 0xc0, 0xe6, 0x04, 0xb0, 0x20, 0xe6, 0x04, 0x30, 0xc0,
            0xe6, 0x81, 0xb0, 0xff, 0xe6, 0x05, 0xb0, 0x01, 0xe6, 0x05, 0xb0, 0x02,
            0xe6, 0x0a, 0xbe, 0x8c, 0x7c, 0xb9, 0x09, 0x00, 0xba, 0xf4, 0x03, 0xec,
            0xa8, 0x80, 0x74, 0xfb, 0x42, 0xac, 0xee, 0xe2, 0xf3, 0xb9, 0x07, 0x00,
            0xba, 0xf4, 0x03, 0xec, 0x24, 0xc0, 0x3c, 0xc0, 0x75, 0xf9, 0x42, 0xec,
            0xe2, 0xf2, 0xff, 0x06, 0x95, 0x7c, 0xeb, 0x81, 0x46, 0x00, 0x00, 0x00,
            0x01, 0x02, 0x01, 0x1b, 0xff, 0x00, 0x00,
        ];
        let floppy: Vec<u8> = (0..1_474_560u32).map(|i| (i % 251) as u8).collect();
        let mut config = busy_drives_config();
        let disk = assembly::bios(vec![0xF4; 32768], Some(floppy));
        // The DMA controllers, the wired floppy controller and its disk come
        // from the floppy-booting board; the IDE channel stays.
        config
            .devices
            .retain(|device| !matches!(device.id, DeviceId::Fdc));
        for device in disk.devices {
            if matches!(device.id, DeviceId::Dma | DeviceId::DmaWord | DeviceId::Fdc) {
                let mut device = device;
                if device.id == DeviceId::Fdc {
                    device.ports = assembly::FdcCfg::port_windows(true);
                }
                config.devices.push(device);
            }
        }
        config
            .drives
            .retain(|drive| drive.controller != DeviceId::Fdc);
        config.drives.extend(
            disk.drives
                .into_iter()
                .filter(|drive| drive.controller == DeviceId::Fdc),
        );
        config.images = assembly::at_boot(code).images;
        config
    }

    #[test]
    fn drive_sampling_leaves_disk_and_dma_guests_identical_in_any_chunking() {
        let config = disk_and_dma_guest_config();
        let mut paced = test_presenter(&config).unwrap();
        let mut seen = [false; drive_chips::DRIVES];
        for millis in [8, 16, 17, 33, 50, 66, 83, 100] {
            paced.advance_fully(Duration::from_millis(millis)).unwrap();
            assert!(paced.running);
            for (seen, busy) in seen.iter_mut().zip(paced.machine().drive_activity()) {
                *seen |= busy;
            }
            paced.observe_drive_lights(Duration::from_millis(millis));
        }
        assert_eq!(
            seen,
            [true, false, true, false],
            "A: and C: were sampled busy"
        );
        let mut rounds = [0; 2];
        paced
            .assembled()
            .machine
            .cpu()
            .read_mem(0x7C95, &mut rounds)
            .unwrap();
        assert!(
            u16::from_le_bytes(rounds) >= 3,
            "the guest finished several IDE and floppy rounds"
        );
        let final_time = paced.assembled().machine.virtual_time();
        let expected = guest_state(paced.assembled());
        for chunk in [137, 997] {
            let mut headless = observed_machine(&config);
            run_to(&mut headless, final_time, chunk);
            assert_eq!(guest_state(&mut headless), expected, "chunk {chunk}");
        }
    }

    #[test]
    fn drive_busy_is_sampled_at_run_boundaries_and_lights_hold_in_host_time() {
        let ms = Duration::from_millis;
        let mut presenter = test_presenter(&busy_drives_config()).unwrap();
        assert_eq!(
            presenter.machine().drive_activity(),
            [false; drive_chips::DRIVES]
        );
        presenter.advance_fully(ms(20)).unwrap();
        assert!(presenter.running);
        assert_eq!(
            presenter.machine().drive_activity(),
            [true, false, true, false]
        );
        presenter.observe_drive_lights(ms(20));
        assert_eq!(
            presenter.machine().drive_activity(),
            [false; drive_chips::DRIVES]
        );
        let lit = |presenter: &Presenter, now| -> Vec<bool> {
            (0..drive_chips::DRIVES)
                .map(|slot| presenter.drive_lights.lit(slot, now))
                .collect()
        };
        assert_eq!(lit(&presenter, ms(20)), [true, false, true, false]);
        // The host stops the motor and takes the sector, so later boundaries are idle.
        let bus = presenter.assembled().machine.bus_mut();
        bus.port_write(0x3F2, 1, 0x0C).unwrap();
        for _ in 0..256 {
            bus.port_read(0x1F0, 2).unwrap();
        }
        presenter.advance_fully(ms(40)).unwrap();
        assert_eq!(
            presenter.machine().drive_activity(),
            [false; drive_chips::DRIVES]
        );
        presenter.observe_drive_lights(ms(40));
        assert_eq!(lit(&presenter, ms(219)), [true, false, true, false]);
        assert_eq!(lit(&presenter, ms(220)), [false; drive_chips::DRIVES]);
        let views = presenter.drive_views(&drive_chips::Origin::MachineFile("test".into()));
        assert_eq!(
            views
                .iter()
                .map(drive_chips::DriveView::chip_text)
                .collect::<Vec<_>>(),
            [
                "A: disk 1 · 1.44M · volatile",
                "C: inline image · read-only"
            ]
        );
    }

    #[test]
    fn window_ide_run_error_flushes_whole_outsw_prefix_without_opening_a_window() {
        let temp = assembly::test_support::TempDir::new("window-ide-error");
        let base_path = temp.path().join("base.img");
        let journal = temp.path().join("disk.upfj");
        let base = vec![0; 32768];
        std::fs::write(&base_path, &base).unwrap();
        let mut code = vec![0xFA, 0xFC];
        for (port, byte) in [
            (0x1F6u16, 0xE0),
            (0x1F2, 3),
            (0x1F3, 4),
            (0x1F4, 0),
            (0x1F5, 0),
            (0x1F7, 0x30),
        ] {
            code.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, byte, 0xEE]);
        }
        code.extend([
            0xBE, 0, 0x20, 0xBA, 0xF0, 1, 0xB9, 0x80, 1, 0xF3, 0x6F, 0xEC,
        ]);
        let demo = assembly::at_boot(code);
        let mut config = assembly::bios(vec![0xF4; 32768], None);
        config.board = assembly::BoardModel::Bare;
        config.cpu = demo.cpu;
        config.images = demo.images;
        config.images.push(assembly::LoadImage {
            base: 0x2000,
            source: assembly::ImageSource::Inline(vec![0xA7; 1536]),
            kind: assembly::ImageKind::RamLoad,
            provides: vec![],
        });
        let ide = assembly::IdeCfg {
            model: assembly::AtaControllerModel::Ata,
            base: 0x1F0,
            control: 0x3F6,
        };
        config.devices.push(
            assembly::DeviceConfig::new(
                DeviceId::Ide,
                assembly::DeviceKind::Ide(ide),
                ide.port_windows(),
            )
            .with_irq(ide.irq()),
        );
        config.drives.push(assembly::DriveConfig {
            media: None,
            controller: DeviceId::Ide,
            unit: 0,
            kind: assembly::DriveKind::HardDisk {
                geometry: assembly::HardDiskGeometry {
                    cylinders: 8,
                    heads: 2,
                    sectors_per_track: 4,
                    bytes_per_sector: 512,
                },
            },
            image: assembly::ImageSource::File(base_path.clone()),
            expected_sha256: None,
            write: assembly::WriteMode::journal(&journal),
        });
        let mut presenter = test_presenter(&config).unwrap();
        presenter.advance_fully(Duration::from_millis(20)).unwrap();
        assert!(presenter.error.is_some());
        assert!(!presenter.running);
        assert!(presenter.window.is_none());
        assert_eq!(std::fs::read(&journal).unwrap().len(), 128 + 2 * 640);
        drop(presenter);
        let mut fresh = assembly::assemble(&config).unwrap();
        let report = fresh.persistence[0].mount_report();
        assert!(report.generations.is_empty());
        assert_eq!(report.unsealed_tip.as_ref().unwrap().commands, 1);
        let ide_index = fresh.device_index[&DeviceId::Ide];
        let bus = fresh.machine.bus_mut();
        for (port, byte) in [
            (0x1F6, 0xE0),
            (0x1F2, 64),
            (0x1F3, 0),
            (0x1F4, 0),
            (0x1F5, 0),
            (0x1F7, 0x20),
        ] {
            bus.port_write(port, 1, byte).unwrap();
        }
        for lba in 0..64 {
            if let device::NextEvent::At(at) = bus
                .device(ide_index)
                .unwrap()
                .next_event(device::VClock::ZERO)
            {
                bus.tick_all(at);
            }
            for _ in 0..256 {
                assert_eq!(
                    bus.port_read(0x1F0, 2).unwrap(),
                    if lba == 4 { 0xA7A7 } else { 0 }
                );
            }
        }
        assert_eq!(std::fs::read(base_path).unwrap(), base);
    }

    #[test]
    fn journal_host_failure_stops_at_the_first_window_boundary() {
        for fail in [false, true] {
            let temp = assembly::test_support::TempDir::new("window-host-failure");
            let base = temp.path().join("base.img");
            let journal = temp.path().join("disk.upfj");
            std::fs::write(&base, vec![0; 1_474_560]).unwrap();
            let out = |program: &mut Vec<u8>, port: u16, value: u8| {
                program.extend([0xBA, port as u8, (port >> 8) as u8, 0xB0, value, 0xEE]);
            };
            let mut program = vec![0xFA, 0xFC];
            out(&mut program, 0x3F2, 0x1C);
            for (port, value) in [
                (0x0A, 6),
                (0x0C, 0),
                (0x0B, 0x4A),
                (4, 0),
                (4, 6),
                (0x81, 0),
                (5, 0xFF),
                (5, 1),
                (0x0A, 2),
            ] {
                out(&mut program, port, value);
            }
            for value in [5, 0, 0, 0, 1, 2, 1, 0x1B, 0xFF] {
                out(&mut program, 0x3F5, value);
            }
            program.extend([0xBF, 0, 0x10]);
            for _ in 0..7 {
                program.extend([0xEC, 0xAA]);
            }
            program.extend([0xB0, 0xFE, 0xE6, 0x64, 0xB0, 0x55, 0xF4]);
            let demo = assembly::at_fdc_write(program, vec![0xA7; 512], 0x600);
            let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
            config.board = assembly::BoardModel::Bare;
            config.cpu = demo.cpu;
            config.images = demo.images;
            config.images.push(assembly::LoadImage {
                base: 0xFFFF0,
                source: assembly::ImageSource::Inline(vec![0xB0, 0x66, 0xF4]),
                kind: assembly::ImageKind::Rom,
                provides: Vec::new(),
            });
            config.drives[0].image = assembly::ImageSource::File(base);
            let mut reference = test_presenter(&config).unwrap();
            assert_eq!(
                reference
                    .assembled()
                    .machine
                    .run_bounded(Some(10_000))
                    .unwrap(),
                machine::RunOutcome::ResetRequested
            );
            let boundary = reference.assembled().machine.virtual_time();
            config.drives[0].write = assembly::WriteMode::journal(&journal);
            let mut presenter = test_presenter(&config).unwrap();
            let mut diagnostics = Vec::new();
            if fail {
                presenter.assembled().persistence[0].fail_output();
                let error = presenter
                    .advance_fully_with_warnings(Duration::from_millis(20), &mut diagnostics)
                    .unwrap_err();
                assert!(error.to_string().contains("drive 0 persistence"), "{error}");
                assert_eq!(presenter.assembled().machine.virtual_time(), boundary);
                assert_eq!(presenter.assembled().machine.cpu().code_segment(), 0);
                assert!(presenter.advance_fully(Duration::from_millis(40)).is_err());
                assert_eq!(presenter.assembled().machine.virtual_time(), boundary);
                assert_eq!(std::fs::read(journal).unwrap().len(), 128);
            } else {
                presenter
                    .advance_fully_with_warnings(Duration::from_millis(20), &mut diagnostics)
                    .unwrap();
                assert_eq!(presenter.assembled().machine.virtual_time(), boundary + 2);
                assert_eq!(presenter.assembled().machine.cpu().code_segment(), 0xF000);
                assert_eq!(
                    cpu::Cpu::reg(presenter.assembled().machine.cpu(), cpu::CpuReg::Rax) & 0xFF,
                    0x66
                );
                assert!(!presenter.running);
            }
            assert_eq!(presenter.machine().reset_requests(), 1);
            assert_eq!(presenter.machine().cpu_resets(), u64::from(!fail));
            if fail {
                presenter.write_reset_summary(&mut diagnostics);
                assert!(diagnostics.is_empty());
            } else {
                assert_eq!(diagnostics, b"cpu reset\n");
            }
            assert!(presenter.window.is_none());
        }
    }

    #[test]
    fn journal_presenter_keeps_a_halt_open_until_stop_and_preserves_unsealed_execution_errors() {
        for fail in [false, true] {
            let temp = assembly::test_support::TempDir::new("journal-test");
            let path = temp.path();
            let base = path.join("base.img");
            let journal = path.join("disk.upfj");
            std::fs::write(&base, vec![0; 1_474_560]).unwrap();
            let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
            config.board = assembly::BoardModel::Bare;
            // A source-free Application rig keeps this host-error-path fixture
            // independent of in-guest processor-exception delivery.
            config.devices.retain(|device| {
                !matches!(
                    device.kind,
                    assembly::DeviceKind::Keyboard(_) | assembly::DeviceKind::SysCtrlPortA
                )
            });
            config.cpu = assembly::at_boot(Vec::new()).cpu;
            config.images = assembly::at_boot(if fail {
                vec![0x0F, 0x05]
            } else {
                vec![0xFA, 0xF4]
            })
            .images;
            config.drives[0].image = assembly::ImageSource::File(base);
            config.drives[0].write = assembly::WriteMode::journal(&journal);
            let mut presenter = test_presenter(&config).unwrap();
            presenter.advance_fully(Duration::from_millis(20)).unwrap();
            assert!(!presenter.running);
            assert_eq!(presenter.error.is_some(), fail);
            assert_eq!(presenter.halted, !fail);
            // The halt leaves the session open for a reset; Stop finishes it.
            assert_eq!(std::fs::read(&journal).unwrap().len(), 128);
            if !fail {
                presenter.finish_persistence(&mut io::sink()).unwrap();
            }
            let bytes = std::fs::read(journal).unwrap();
            assert_eq!(bytes.len(), if fail { 128 } else { 128 + 640 });
            if !fail {
                assert_eq!(bytes[128 + 8], 3);
            }
            drop(presenter);
        }
    }

    #[test]
    fn floppy_media_window_prints_retention_warning_after_same_boundary_reinsert() {
        let temp = assembly::test_support::TempDir::new("window-removable-warning");
        let base = temp.path().join("base.img");
        std::fs::write(&base, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
        config.board = assembly::BoardModel::Bare;
        let demo = assembly::at_boot(vec![0xFA, 0xF4]);
        config.cpu = demo.cpu;
        config.images = demo.images;
        config.drives[0].image = assembly::ImageSource::File(base);
        config.drives[0].write = assembly::WriteMode::Journal {
            path: temp.path().join("disk.upfj"),
            keep_generations: 1,
        };
        let mut presenter = test_presenter(&config).unwrap();
        let assembled = presenter.assembled();
        assembled.persistence[0]
            .checkpoint(&mut assembled.machine)
            .unwrap();
        presenter.assembled().persistence[0].fail_retention("temporary write");
        presenter
            .assembled()
            .change_floppy(DeviceId::Fdc, 0, assembly::FloppyAction::Insert(0))
            .unwrap();
        let mut warnings = Vec::new();
        presenter.finish_persistence(&mut warnings).unwrap();
        let expected = "warning: drive 0: retention maintenance failed before publication: injected retention write failure\n";
        assert_eq!(String::from_utf8(warnings.clone()).unwrap(), expected);
        presenter.finish_persistence(&mut warnings).unwrap();
        assert_eq!(String::from_utf8(warnings).unwrap(), expected);
        assert!(presenter.window.is_none());
    }

    #[test]
    fn window_finishes_every_drive_and_warns_once_for_maintenance() {
        for operation in ["temporary write", "publication directory sync"] {
            for both_fail in [false, true] {
                let temp = assembly::test_support::TempDir::new("window-maintenance");
                let base = temp.path().join("base.img");
                std::fs::write(&base, vec![0; 1_474_560]).unwrap();
                let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
                config.board = assembly::BoardModel::Bare;
                let demo = assembly::at_boot(vec![0xFA, 0xF4]);
                config.cpu = demo.cpu;
                config.images = demo.images;
                config.drives[0].image = assembly::ImageSource::File(base);
                config.drives[0].write = assembly::WriteMode::Journal {
                    path: temp.path().join("first.upfj"),
                    keep_generations: 1,
                };
                let mut second = config.drives[0].clone();
                second.unit = 1;
                second.write = assembly::WriteMode::Journal {
                    path: temp.path().join("second.upfj"),
                    keep_generations: if both_fail { 1 } else { 10 },
                };
                config.drives.push(second);
                let mut presenter = test_presenter(&config).unwrap();
                let assembled = presenter.assembled();
                for handle in &mut assembled.persistence {
                    handle.checkpoint(&mut assembled.machine).unwrap();
                }
                presenter.assembled().persistence[0].fail_retention(operation);
                if both_fail {
                    presenter.assembled().persistence[1].fail_retention("temporary write");
                }
                let mut warnings = Vec::new();
                presenter
                    .advance_fully_with_warnings(Duration::from_millis(20), &mut warnings)
                    .unwrap();
                assert!(!presenter.running);
                assert!(presenter.error.is_none());
                // The halted session ends at Stop, which reports maintenance.
                assert!(presenter.halted);
                assert!(warnings.is_empty());
                presenter.finish_persistence(&mut warnings).unwrap();
                let expected = if operation == "temporary write" {
                    "warning: drive 0: retention maintenance failed before publication: injected retention write failure\n"
                } else {
                    "warning: drive 0: retention maintenance completion uncertain; reopen required: injected retention barrier failure\n"
                };
                let expected = if both_fail {
                    format!("{expected}warning: drive 1: retention maintenance failed before publication: injected retention write failure\n")
                } else {
                    expected.to_owned()
                };
                assert_eq!(String::from_utf8(warnings.clone()).unwrap(), expected);
                presenter.finish_persistence(&mut warnings).unwrap();
                assert_eq!(String::from_utf8(warnings).unwrap(), expected);
                let bytes = std::fs::read(temp.path().join("second.upfj")).unwrap();
                assert_eq!(bytes.len(), 128 + 2 * 640);
                assert_eq!(bytes[bytes.len() - 640 + 32], 1);
                assert!(presenter.window.is_none());
            }
        }
    }

    const TEST_FRAME_CLOCKS: u64 = 14_318;

    fn advance_frame(presenter: &mut Presenter) {
        let target = presenter.assembled().machine.clock().0 + TEST_FRAME_CLOCKS;
        let frequency = device::clock::PIT_HZ;
        let nanos = (u128::from(target) * u128::from(frequency.denominator()) * 1_000_000_000)
            .div_ceil(
                u128::from(frequency.numerator()) * u128::from(device::clock::TICKS_PER_PIT_CLOCK),
            );
        presenter
            .advance_fully(Duration::from_nanos(nanos as u64))
            .unwrap();
    }

    #[test]
    fn informational_options_carry_the_embedded_font_notice() {
        for option in ["--help", "--version", "--about"] {
            let output = informational_output(OsStr::new(option)).unwrap();
            assert!(output.contains(FONT_NOTICE), "{option}");
            assert!(output.contains("unemupc"));
        }
        assert!(informational_output(OsStr::new("bios.bin")).is_none());
    }

    #[test]
    fn version_and_about_name_the_licence_source_and_opl3_notice() {
        for option in ["--version", "--about"] {
            let output = informational_output(OsStr::new(option)).unwrap();
            for expected in [
                "AGPL-3.0-or-later",
                "https://github.com/unemu-org/unemupc",
                "Nuked-OPL3",
                "LGPL-2.1-or-later",
                "THIRD-PARTY-LICENSES.md",
            ] {
                assert!(output.contains(expected), "{option}: {expected}");
            }
        }
    }

    #[test]
    fn machine_files_come_from_the_switch_then_cargo_then_the_platform_data_directory() {
        let data = || Some(PathBuf::from("/data/user"));
        let crate_dir = || Some(std::ffi::OsString::from("/src/tree/crates/window"));
        let from_cargo = PathBuf::from("/src/tree/crates/window/../../machines");
        let from_data = PathBuf::from("/data/user/unemupc/machines");
        assert_eq!(
            machines_dir_from(Some("/srv/machines".into()), crate_dir(), data()),
            Some(PathBuf::from("/srv/machines"))
        );
        for switch in [None, Some("".into())] {
            assert_eq!(
                machines_dir_from(switch.clone(), crate_dir(), data()),
                Some(from_cargo.clone())
            );
            assert_eq!(
                machines_dir_from(switch.clone(), Some("".into()), data()),
                Some(from_data.clone())
            );
            assert_eq!(
                machines_dir_from(switch, None, data()),
                Some(from_data.clone())
            );
        }
        assert_eq!(machines_dir_from(None, None, None), None);
        assert_eq!(
            default_machine(),
            machines_dir().map(|directory| directory.join("at-freedos.toml"))
        );
        let help = informational_output(OsStr::new("--help")).unwrap();
        assert!(help.contains(&format!(
            "machine files: {} (set UNEMUPC_MACHINES_DIR to use another directory)",
            shown_path(machines_dir())
        )));
        assert_eq!(shown_path(None), "none (the host has no data directory)");
    }

    /// A release binary runs far from the tree it was built in, so neither the
    /// default machine nor the notices may point into that tree. `cargo test`
    /// names the crate in CARGO_MANIFEST_DIR at run time, so the release
    /// environment is passed in without it.
    #[test]
    fn default_machine_does_not_point_into_the_build_tree() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = manifest.join("../..").canonicalize().unwrap();
        let inside = |path: &Path| {
            path.starts_with(manifest)
                || path.starts_with(&root)
                || path
                    .canonicalize()
                    .is_ok_and(|path| path.starts_with(&root))
        };
        // Without any data directory there is no default at all.
        let data = image_store::location::data_directory().ok();
        if let Some(released) = machines_dir_from(None, None, data) {
            assert!(!inside(&released), "{}", released.display());
            assert!(!inside(&released.join("at-freedos.toml")));
        }
        for option in ["--version", "--about"] {
            let output = informational_output(OsStr::new(option)).unwrap();
            assert!(!output.contains(env!("CARGO_MANIFEST_DIR")), "{option}");
            assert!(!output.contains(&*root.to_string_lossy()), "{option}");
        }
    }

    #[test]
    fn default_fixture_paths_are_absolute_and_workspace_relative() {
        for relative in [BIOS_FIXTURE, FLOPPY_FIXTURE] {
            let path = default_fixture(relative);
            assert!(path.is_absolute());
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            assert_eq!(path.strip_prefix(root).unwrap(), Path::new(relative));
        }
    }

    #[test]
    fn missing_fixtures_name_the_path_and_fetch_script() {
        for (path, fetch) in [
            ("missing-window-bios.bin", BIOS_FETCH),
            ("missing-window-floppy.img", FLOPPY_FETCH),
        ] {
            let error = read_fixture(Path::new(path), fetch).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            let message = error.to_string();
            assert!(message.contains(path));
            assert!(message.contains(fetch));
        }
    }

    fn observed_machine(config: &assembly::MachineConfig) -> AssembledMachine {
        assemble_with_observation(
            config,
            &ObservationPolicy {
                routes: vec![ObservationRequest {
                    device: DeviceId::Mda,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..ObservationPolicy::default()
            },
        )
        .unwrap()
    }

    fn fixture_result(
        result: io::Result<Vec<u8>>,
        path: &Path,
        fetch: &str,
        required: Option<&OsStr>,
    ) -> Option<Vec<u8>> {
        match result {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let message = format!("missing fixture {} — run {fetch}", path.display());
                assert!(required != Some(OsStr::new("1")), "{message}");
                eprintln!("skipping: {message}");
                None
            }
            Err(error) => panic!("cannot read fixture {}: {error}", path.display()),
        }
    }

    #[test]
    fn fixture_policy_requires_only_one_and_skips_only_not_found() {
        for required in [
            None,
            Some(OsStr::new("")),
            Some(OsStr::new("0")),
            Some(OsStr::new("true")),
            Some(OsStr::new("1")),
        ] {
            assert_eq!(
                fixture_result(Ok(vec![1]), Path::new("fixture"), "fetch", required),
                Some(vec![1])
            );
            let missing = std::panic::catch_unwind(|| {
                fixture_result(
                    Err(io::Error::from(io::ErrorKind::NotFound)),
                    Path::new("fixture"),
                    "fetch",
                    required,
                )
            });
            assert_eq!(missing.is_err(), required == Some(OsStr::new("1")));
            for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::InvalidData] {
                assert!(std::panic::catch_unwind(|| fixture_result(
                    Err(io::Error::from(kind)),
                    Path::new("fixture"),
                    "fetch",
                    required
                ))
                .is_err());
            }
        }
    }

    #[test]
    fn stopped_or_errored_presenter_ignores_keys_and_focus_loss_releases_held_keys() {
        let mut presenter = test_presenter(&assembly::bios(vec![0; 0x8000], None)).unwrap();
        presenter.running = false;
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Pressed);
        assert_eq!(presenter.keyboard().pending_events(), 0);
        presenter.running = true;
        presenter.error = Some("presentation failed".into());
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Pressed);
        assert_eq!(presenter.keyboard().pending_events(), 0);
        presenter.error = None;
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Pressed);
        presenter.focus_changed(true);
        assert_eq!(presenter.keyboard().pending_events(), 1);
        presenter.focus_changed(false);
        assert_eq!(presenter.keyboard().pending_events(), 2);
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Released);
        assert_eq!(presenter.keyboard().pending_events(), 2);
    }

    #[test]
    fn synthetic_date_prompt_keeps_running_and_accepts_a_key() {
        // Write the complete DOS prompt, then poll the 8042 and display its byte.
        let prompt = b"Enter new date (mm-dd-[cc]yy):";
        let mut program = vec![0xB8, 0, 0xB0, 0x8E, 0xD8];
        for (cell, byte) in prompt.iter().enumerate() {
            program.extend([0xC6, 0x06, (cell * 2) as u8, 0, *byte]);
        }
        program.extend([
            0xE4,
            0x64,
            0xA8,
            1,
            0x74,
            0xFA,
            0xE4,
            0x60,
            0xA2,
            (prompt.len() * 2) as u8,
            0,
            0xEB,
            0xFE,
        ]);
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = assembly::SegmentSeed::flat(0);
        let mut presenter = test_presenter(&config).unwrap();
        for _ in 0..2 {
            advance_frame(&mut presenter);
            assert!(presenter.running, "a prompt is not a machine stop");
        }
        let video::Frame::Text(frame) = presenter
            .assembled()
            .capture_text(DeviceId::Mda)
            .unwrap()
            .frame
        else {
            panic!("expected text frame")
        };
        assert!(frame
            .to_dump()
            .starts_with(std::str::from_utf8(prompt).unwrap()));
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        advance_frame(&mut presenter);
        assert!(presenter.running);
        assert_eq!(
            presenter.assembled().machine.virtual_time(),
            3 * TEST_FRAME_CLOCKS
        );
        let video::Frame::Text(frame) = presenter
            .assembled()
            .capture_text(DeviceId::Mda)
            .unwrap()
            .frame
        else {
            panic!("expected text frame")
        };
        assert!(frame.to_dump().contains("Enter new date (mm-dd-[cc]yy):!"));
        assert!(presenter.picture().is_some());
        assert!(presenter.window.is_none());
    }

    #[test]
    fn reset_diagnostics_log_only_the_first_performed_reset_and_the_session_total() {
        let config = assembly::bios(vec![0; 0x8000], None);
        let mut presenter = test_presenter(&config).unwrap();
        let mut diagnostics = Vec::new();
        presenter.write_reset_summary(&mut diagnostics);
        assert!(diagnostics.is_empty());
        for _ in 0..10_000 {
            presenter.machine_mut().record_cpu_reset();
        }
        let mut messages = Vec::new();
        unemu_core_api::Machine::drain_messages(presenter.machine_mut(), &mut messages);
        for message in messages {
            io::Write::write_all(&mut diagnostics, format!("{}\n", message.text).as_bytes())
                .unwrap();
        }
        assert_eq!(diagnostics, b"cpu reset\n");
        assert_eq!(presenter.machine().cpu_resets(), 10_000);
        presenter.write_reset_summary(&mut diagnostics);
        assert_eq!(diagnostics, b"cpu reset\ncpu resets in session: 10000\n");
        assert!(presenter.window.is_none());
    }

    #[test]
    fn a_guest_reset_loop_counts_requests_and_keeps_running() {
        let mut rom = vec![0; 0x8000];
        rom[..6].copy_from_slice(&[0xB0, 0xFE, 0xE6, 0x64, 0xEB, 0xFC]);
        rom[0x7FF0..0x7FF5].copy_from_slice(&[0xEA, 0, 0, 0, 0xF8]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        advance_frame(&mut presenter);
        assert!(presenter.machine().reset_requests() > 1_000);
        assert!(presenter.running);
        assert_eq!(
            presenter.assembled().machine.virtual_time(),
            TEST_FRAME_CLOCKS
        );
        let count = presenter.machine().reset_requests();
        advance_frame(&mut presenter);
        assert!(presenter.machine().reset_requests() > count);
        let mut summary = Vec::new();
        presenter.write_reset_summary(&mut summary);
        assert_eq!(
            summary,
            format!(
                "cpu resets in session: {}\n",
                presenter.machine().cpu_resets()
            )
            .as_bytes()
        );
        assert!(presenter.window.is_none());
    }

    #[test]
    fn window_restarts_at_the_cpu_reset_vector() {
        use cpu::{Cpu, CpuReg};
        let mut rom = vec![0; 0x8000];
        rom[..5].copy_from_slice(&[0xB0, 0xFE, 0xE6, 0x64, 0xF4]);
        rom[0x7FF0..0x7FF4].copy_from_slice(&[0xB0, 0x55, 0xFA, 0xF4]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        advance_frame(&mut presenter);
        assert_eq!(
            presenter.assembled().machine.cpu().reg(CpuReg::Rax) & 0xFF,
            0x55
        );
        assert_eq!(presenter.assembled().machine.virtual_time(), 5);
        assert!(!presenter.running);
        assert!(presenter.window.is_none());
    }

    #[test]
    fn host_queue_only_delivers_at_chunk_start_and_defers_full_buffer_in_order() {
        use winit::{event::ElementState::Pressed, keyboard::KeyCode};
        // Sample OBF before polling, so injection halfway through a chunk is visible.
        #[rustfmt::skip]
        let program = [
            0xB8, 0, 0xB0, 0x8E, 0xD8,
            0xE4, 0x64, 0x24, 1, 0x04, 0x30, 0xA2, 0, 0, 0xBB, 2, 0,
            0xE4, 0x64, 0xA8, 1, 0x74, 0xFA,
            0xE4, 0x60, 0x88, 0x07, 0x83, 0xC3, 2, 0xEB, 0xF1,
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = assembly::SegmentSeed::flat(0);
        let mut presenter = test_presenter(&config).unwrap();
        presenter.guest_key(KeyCode::KeyF, Pressed);
        presenter.guest_key(KeyCode::KeyG, Pressed);
        assert_eq!(presenter.keyboard().pending_events(), 2);
        assert_eq!(presenter.assembled().machine.virtual_time(), 0);
        assert_eq!(
            presenter
                .assembled()
                .machine
                .bus_mut()
                .port_read(0x64, 1)
                .unwrap()
                & 1,
            0
        );
        let mut checks = 0;
        presenter
            .advance_with_slice_clock(Duration::from_millis(12), &mut io::sink(), || {
                checks += 1;
                if checks == 1 {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                }
            })
            .unwrap();
        let first_time = presenter.assembled().machine.virtual_time();
        assert!(first_time > 0);
        assert_eq!(presenter.keyboard().pending_events(), 1);
        let first = presenter.assembled().capture_text(DeviceId::Mda).unwrap();
        let video::Frame::Text(first) = first.frame else {
            panic!("expected text frame")
        };
        assert!(first.to_dump().starts_with("1!"));
        assert!(
            !first.to_dump().starts_with("1!\""),
            "second byte must wait for another boundary"
        );
        advance_frame(&mut presenter);
        assert_eq!(presenter.keyboard().pending_events(), 0);
        let second = presenter.assembled().capture_text(DeviceId::Mda).unwrap();
        let video::Frame::Text(second) = second.frame else {
            panic!("expected text frame")
        };
        assert!(second.to_dump().starts_with("1!\""));
        assert_eq!(
            presenter.assembled().machine.virtual_time(),
            first_time + TEST_FRAME_CLOCKS
        );
        assert!(presenter.picture().is_some());
        assert!(presenter.window.is_none());
    }

    #[test]
    fn tagged_reset_chords_replay_through_the_inhibited_keyboard_buffer() {
        let config = assembly::at_keyboard(vec![0xEB, 0xFE]);
        let mut original = assembly::assemble(&config).unwrap();
        original
            .machine
            .bus_mut()
            .port_write(0x64, 1, 0xAD)
            .unwrap();
        let mut keyboard = unemu_pc_core::HostKeyboard::default();
        keyboard.ctrl_alt_del();
        let bytes = keyboard.inject_at_boundary(&mut original);
        let log = vec![(0, bytes, keyboard.typed_ranges().to_vec())];
        assert_eq!(log[0].2.len(), 1);
        assert_eq!(log[0].2[0], 0..6);
        let mut replay = assembly::assemble(&config).unwrap();
        replay.machine.bus_mut().port_write(0x64, 1, 0xAD).unwrap();
        replay_input(&mut replay, &log, 0, 1);
        let index = original.device_index[&DeviceId::Keyboard];
        assert_eq!(
            original.machine.bus().device(index).unwrap().save(),
            replay.machine.bus().device(index).unwrap().save()
        );
    }

    fn replay_input(
        assembled: &mut AssembledMachine,
        log: &[unemu_pc_core::InputLogEntry],
        final_time: u64,
        chunk: u64,
    ) {
        let keyboard = assembled.device_index[&DeviceId::Keyboard];
        for (time, bytes, typed) in log {
            run_to(assembled, *time, chunk);
            let mut offset = 0;
            while offset < bytes.len() {
                if let Some(range) = typed.iter().find(|range| range.start == offset) {
                    assert!(assembled.type_scancodes(&bytes[range.clone()]));
                    offset = range.end;
                } else {
                    assert!(assembly::try_inject_scancode(
                        &mut assembled.machine,
                        keyboard,
                        bytes[offset]
                    ));
                    offset += 1;
                }
            }
        }
        run_to(assembled, final_time, chunk);
    }

    fn run_to(assembled: &mut AssembledMachine, time: u64, chunk: u64) {
        while assembled.machine.virtual_time() < time {
            let cap = (assembled.machine.virtual_time() + chunk).min(time);
            assert_eq!(
                assembled.machine.run_bounded(Some(cap)).unwrap(),
                machine::RunOutcome::RetiredCapReached
            );
        }
        assert_eq!(assembled.machine.virtual_time(), time);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct GuestState {
        frame: video::Frame,
        clock: u64,
        virtual_time: u64,
        retired: u64,
        registers: [u64; 9],
        cs: u64,
        // The seam exposes IF, but not the remaining FLAGS bits.
        interrupts_enabled: bool,
        ram_hash: u64,
        devices: Vec<Vec<u8>>,
    }

    fn guest_state(assembled: &mut AssembledMachine) -> GuestState {
        use cpu::{Cpu, CpuReg::*};
        use std::hash::{Hash, Hasher};
        assembled.machine.bus_mut().settle();
        let devices = (0..assembled.device_index.len())
            .map(|index| assembled.machine.bus().device(index).unwrap().save())
            .collect();
        let cpu = assembled.machine.cpu();
        let mut ram = vec![0; 0xA0000];
        cpu.read_mem(0, &mut ram).unwrap();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        ram.hash(&mut hash);
        GuestState {
            frame: assembled.capture_text(DeviceId::Mda).unwrap().frame,
            clock: assembled.machine.clock().0,
            virtual_time: assembled.machine.virtual_time(),
            retired: assembled.machine.cpu().retired(),
            registers: [Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, Rip]
                .map(|reg| assembled.machine.cpu().reg(reg)),
            cs: assembled.machine.cpu().code_segment(),
            interrupts_enabled: assembled.machine.cpu().interrupts_enabled(),
            ram_hash: hash.finish(),
            devices,
        }
    }

    #[test]
    fn speed_paced_frame_reaches_target_after_eight_host_milliseconds() {
        let target = pacing::virtual_ticks(Duration::from_millis(8)) as u64;
        for (mode, millis) in [
            (Speed::Normal, 8),
            (Speed::Double, 4),
            (Speed::Quadruple, 2),
        ] {
            let mut presenter = test_presenter(&timer_guest_config()).unwrap();
            presenter.select_speed(mode, Duration::ZERO);
            let mut host_time = Duration::ZERO;
            presenter
                .advance_with_slice_clock(Duration::from_millis(millis), &mut io::sink(), || {
                    let now = host_time;
                    host_time += Duration::from_millis(1);
                    now
                })
                .unwrap();
            assert!(host_time >= Duration::from_millis(8), "{mode:?}");
            assert!(
                presenter.assembled().machine.clock().0 >= target,
                "{mode:?}"
            );
        }
    }

    #[test]
    fn speed_unlimited_presenter_stops_at_the_host_slice_budget() {
        let mut presenter = test_presenter(&timer_guest_config()).unwrap();
        presenter.select_speed(Speed::Unlimited, Duration::ZERO);
        let period = pacing::next_frame(Duration::ZERO);
        let budget = Speed::Unlimited.slice_budget(period);
        let mut host_time = Duration::ZERO;
        presenter
            .advance_with_slice_clock(Duration::ZERO, &mut io::sink(), || {
                let spent = host_time;
                host_time += Duration::from_millis(1);
                spent
            })
            .unwrap();
        assert!(host_time >= budget);
        assert!(
            host_time < period,
            "Unlimited must yield before the next paint"
        );
        let clock = presenter.assembled().machine.clock().0;
        assert!(clock > 0);
        assert!(clock < pacing::virtual_ticks(period) as u64);
        presenter
            .advance_with_slice_clock(Duration::ZERO, &mut io::sink(), || period)
            .unwrap();
        assert_eq!(presenter.assembled().machine.clock().0, clock);
        presenter.select_speed(Speed::Normal, Duration::from_secs(100));
        presenter
            .advance_with_slice_clock(Duration::from_secs(100), &mut io::sink(), || Duration::ZERO)
            .unwrap();
        assert_eq!(presenter.assembled().machine.clock().0, clock);
    }

    #[test]
    fn speed_modes_replay_the_same_script_at_equal_retired_counts() {
        #[rustfmt::skip]
        let program = [
            0xB8, 0, 0xB0, 0x8E, 0xD8,
            0xBB, 0, 0, 0xB9, 0, 0,
            0x41, 0xE4, 0x64, 0xA8, 1, 0x74, 0xF9,
            0xE4, 0x60, 0x88, 0x07, 0x83, 0xC3, 2,
            0x89, 0x0F, 0x83, 0xC3, 2, 0xEB, 0xEB,
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = assembly::SegmentSeed::flat(0);
        config.clock.retired_per_pit_clock = 1;
        config.clock.retired_per_pit_clock_denominator = 1;
        let script = [(5000, 0x1E), (12000, 0x30), (19000, 0x9E), (26000, 0xB0)];
        let final_time = 60_000;
        let replay = |mode| {
            let mut assembled = observed_machine(&config);
            let keyboard = assembled.device_index[&DeviceId::Keyboard];
            let mut policy = machine_view::pacing::Pacing::new(pacing::PIT);
            policy.select(mode, Duration::ZERO, 0);
            let rate = unemu_pc_core::pacing::CpuRate {
                numerator: std::num::NonZeroU64::new(1).unwrap(),
                denominator: std::num::NonZeroU64::new(1).unwrap(),
            };
            let mut elapsed = Duration::ZERO;
            let mut caps = Vec::new();
            let mut delivered = Vec::new();
            let mut binding_targets = 0;
            let mut frames = 0;
            while assembled.machine.virtual_time() < final_time {
                elapsed += pacing::next_frame(Duration::ZERO);
                let period = policy.frame_period(elapsed);
                let target = policy.target(elapsed, assembled.machine.clock().0);
                let mut spent = Duration::ZERO;
                loop {
                    let time = assembled.machine.virtual_time();
                    if let Some(&(stamp, byte)) = script.get(delivered.len()) {
                        if time == stamp {
                            assert!(assembly::try_inject_scancode(
                                &mut assembled.machine,
                                keyboard,
                                byte
                            ));
                            delivered.push((time, byte));
                        }
                    }
                    let clock = assembled.machine.clock().0;
                    if time == final_time
                        || target.is_some_and(|target| clock >= target)
                        || !policy.slice_open(spent, period)
                    {
                        break;
                    }
                    let input_boundary = script
                        .get(delivered.len())
                        .map_or(final_time, |event| event.0);
                    let cap = unemu_pc_core::pacing::run_cap(
                        policy.chunk_target(clock, target),
                        clock,
                        time,
                        rate,
                    )
                    .min(input_boundary)
                    .min(final_time);
                    assert!(cap > time);
                    caps.push(cap);
                    run_to(&mut assembled, cap, cap - time);
                    spent += Duration::from_micros(100);
                }
                if target.is_some_and(|target| assembled.machine.clock().0 >= target) {
                    binding_targets += 1;
                }
                policy.observe(elapsed, assembled.machine.clock().0);
                frames += 1;
                assert!(frames < 1000);
            }
            assert_eq!(delivered, script);
            let state = guest_state(&mut assembled);
            assert_eq!(state.retired, final_time);
            (state, caps, binding_targets, frames)
        };
        let unlimited = replay(Speed::Unlimited);
        assert_eq!(unlimited.2, 0);
        for mode in [Speed::Normal, Speed::Double] {
            let paced = replay(mode);
            assert!(paced.2 > 0, "the paced target must bind");
            assert!(paced.3 > unlimited.3, "the frame partitions must differ");
            assert_ne!(paced.1, unlimited.1, "the run caps must differ");
            assert_eq!(paced.0, unlimited.0, "mode {mode:?}");
        }
    }

    #[test]
    fn replay_oracle_detects_cpu_and_ram_changes_without_display_changes() {
        // Equal instruction counts and unchanged display/device state leave
        // CPU registers, IF and ordinary RAM as the only distinguishing state.
        let program = [
            0xB8, 0, 0, 0xB9, 0, 0, 0xFA, 0x90, 0xC6, 0x06, 0, 5, 0, 0x90,
        ];
        let capture = |program: &[u8]| {
            let mut rom = vec![0; 0x8000];
            rom[..program.len()].copy_from_slice(program);
            let mut config = assembly::bios(rom, None);
            config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
            config.cpu.segments = assembly::SegmentSeed::flat(0);
            let mut assembled = observed_machine(&config);
            run_to(&mut assembled, 6, 2);
            guest_state(&mut assembled)
        };
        let baseline = capture(&program);
        assert_eq!(baseline.retired, 6);
        assert_eq!(baseline.cs, 0xF800);
        assert_eq!(baseline.registers[8], program.len() as u64);
        for (index, value) in [(1, 0x42), (12, 0x42), (6, 0xFB)] {
            let mut changed = program;
            changed[index] = value;
            let state = capture(&changed);
            assert_eq!(state.frame, baseline.frame);
            assert_eq!(state.clock, baseline.clock);
            assert_eq!(state.virtual_time, baseline.virtual_time);
            assert_eq!(state.devices, baseline.devices);
            assert_ne!(state, baseline, "changed instruction byte {index}");
        }
    }

    #[test]
    fn paced_input_log_replays_headlessly_with_different_chunking() {
        use ElementState::{Pressed, Released};
        // The polling counter is copied beside every scan byte: moving input
        // to a different boundary changes the visible result, even without IRQs.
        #[rustfmt::skip]
        let program = [
            0xB8, 0, 0xB0, 0x8E, 0xD8, // DS = MDA
            0xBB, 0, 0, 0xB9, 0, 0,   // BX = destination, CX = polls
            0x41, 0xE4, 0x64, 0xA8, 1, 0x74, 0xF9,
            0xE4, 0x60, 0x88, 0x07, 0x83, 0xC3, 2,
            0x89, 0x0F, 0x83, 0xC3, 2, 0xEB, 0xEB,
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        for (a, b) in [(1, 1), (5, 2), (2, 3)] {
            let mut config = assembly::bios(rom.clone(), None);
            config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
            config.cpu.segments = assembly::SegmentSeed::flat(0);
            config.clock.retired_per_pit_clock = a;
            config.clock.retired_per_pit_clock_denominator = b;
            let mut paced = test_presenter(&config).unwrap();
            // This is the injected time source used by the real advance path;
            // repeated timestamps, jitter and a long stall need no real sleeps.
            for (index, millis) in [0, 8, 8, 16, 32, 500, 501, 520, 540, 560, 580, 600]
                .into_iter()
                .enumerate()
            {
                match index {
                    0 => paced.key_event(KeyCode::KeyF, Pressed),
                    1 => paced.key_event(KeyCode::KeyG, Pressed),
                    2 => paced.key_event(KeyCode::KeyF, Released),
                    4 => paced.key_event(KeyCode::KeyG, Released),
                    5 => paced.key_event(KeyCode::ArrowRight, Pressed),
                    6 => paced.key_event(KeyCode::ArrowRight, Released),
                    _ => {}
                }
                paced.advance_fully(Duration::from_millis(millis)).unwrap();
                assert!(paced.running);
            }
            assert!(paced.pacing.dropped > 0, "the fake stall must drop backlog");
            assert_eq!(paced.keyboard().pending_events(), 0);
            let bytes: Vec<_> = paced
                .machine()
                .input_log
                .iter()
                .flat_map(|(_, bytes, _)| bytes.clone())
                .collect();
            assert_eq!(bytes, [0x21, 0x22, 0xA1, 0xA2, 0xE0, 0x4D, 0xE0, 0xCD]);
            assert_eq!(paced.machine().input_log[0].0, 0);
            let final_time = paced.assembled().machine.virtual_time();
            let expected = guest_state(paced.assembled());
            for chunk in [137, 997] {
                let mut headless = observed_machine(&config);
                replay_input(&mut headless, &paced.machine().input_log, final_time, chunk);
                assert_eq!(
                    guest_state(&mut headless),
                    expected,
                    "rate {a}/{b}, chunk {chunk}"
                );
            }
            // Prove that this guest/state oracle is sensitive to timing, rather
            // than merely checking that the same byte string eventually arrives.
            let shifted: Vec<_> = paced
                .machine()
                .input_log
                .iter()
                .map(|(time, bytes, typed)| (time + 17, bytes.clone(), typed.clone()))
                .collect();
            let mut wrong = observed_machine(&config);
            replay_input(&mut wrong, &shifted, final_time, 137);
            assert_ne!(guest_state(&mut wrong).frame, expected.frame);
        }
    }

    #[test]
    fn fractional_halt_frames_reach_the_target_without_excess_work() {
        for (a, b) in [(5u64, 2u64), (2, 3), (1, 10)] {
            let mut config = timer_guest_config();
            config.clock.retired_per_pit_clock = a;
            config.clock.retired_per_pit_clock_denominator = b;
            let mut presenter = test_presenter(&config).unwrap();
            for frame in 1..=180 {
                let elapsed = Duration::from_nanos(frame * 16_666_667);
                let (target, _) = pacing::frame_target(
                    elapsed,
                    presenter.assembled().machine.clock().0,
                    presenter.pacing.dropped,
                    1,
                );
                presenter.advance_fully(elapsed).unwrap();
                let clock = presenter.assembled().machine.clock().0;
                assert!(presenter.running);
                assert!(
                    target <= clock && clock <= target + b.div_ceil(a),
                    "rate {a}/{b}, frame {frame}: target {target}, clock {clock}"
                );
            }
        }
    }

    #[test]
    fn hidden_presenter_pauses_and_discards_all_hidden_time() {
        let mut presenter = test_presenter(&timer_guest_config()).unwrap();
        let visible = Duration::from_millis(10);
        presenter.advance_fully(visible).unwrap();
        let before = guest_state(presenter.assembled());
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        presenter.occlusion_changed(true, visible);
        // Repeated notifications cannot shorten the hidden interval.
        presenter.occlusion_changed(true, Duration::from_secs(1));
        let resumed = Duration::from_secs(120);
        presenter.advance_fully(resumed).unwrap();
        assert_eq!(guest_state(presenter.assembled()), before);
        assert_eq!(presenter.keyboard().pending_events(), 1);
        assert!(presenter.machine().input_log.is_empty());
        assert_eq!(
            wake(
                resumed,
                presenter.next_paint,
                presenter.occluded_since.is_none()
            ),
            (false, None)
        );
        presenter.occlusion_changed(false, resumed);
        assert_eq!(presenter.pacing.dropped, 0);
        assert!(wake(resumed, presenter.next_paint, true).0);
        presenter.advance_fully(resumed).unwrap();
        assert_eq!(guest_state(presenter.assembled()), before);
        presenter
            .advance_fully(resumed + Duration::from_millis(10))
            .unwrap();
        assert!(presenter.assembled().machine.clock().0 > before.clock);
        assert_eq!(presenter.keyboard().pending_events(), 0);
    }

    #[test]
    fn identical_host_time_does_not_advance_or_deliver_queued_input() {
        let mut config = assembly::bios(vec![0x90; 0x8000], None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        presenter.advance_fully(Duration::ZERO).unwrap();
        assert_eq!(presenter.assembled().machine.clock().0, 0);
        assert_eq!(presenter.assembled().machine.virtual_time(), 0);
        assert_eq!(presenter.keyboard().pending_events(), 1);
        assert!(presenter.machine().input_log.is_empty());
    }

    #[test]
    #[ignore = "slow: boots FreeDOS then types through the headless window driver"]
    fn freedos_window_chunks_deliver_dir_listing_without_opening_a_window() {
        use std::{cell::RefCell, rc::Rc};
        use video::{Frame, FrameSink};
        use winit::{
            event::ElementState::{Pressed, Released},
            keyboard::KeyCode,
        };
        struct Frames(Rc<RefCell<String>>);
        impl FrameSink for Frames {
            fn present(&mut self, frame: &Frame) {
                let Frame::Text(grid) = frame else {
                    panic!("expected text frame")
                };
                *self.0.borrow_mut() = grid.to_dump();
            }
        }
        let required = std::env::var_os("UNEMUPC_REQUIRE_FIXTURES");
        let fixtures = [(BIOS_FIXTURE, BIOS_FETCH), (FLOPPY_FIXTURE, FLOPPY_FETCH)].map(
            |(relative, fetch)| {
                let path = default_fixture(relative);
                fixture_result(
                    read_fixture(&path, fetch),
                    &path,
                    fetch,
                    required.as_deref(),
                )
            },
        );
        let [Some(rom), Some(floppy)] = fixtures else {
            return;
        };
        let machine = default_fixture("machines/at-freedos.toml");
        let loaded =
            machine_file::load_for_assembly(&machine, assembly::AssemblyOptions::default())
                .unwrap();
        assert_eq!(loaded.config, assembly::bios(rom, Some(floppy)));
        let config = loaded.config;
        let args = parse_args(["--machine".into(), machine.into_os_string()]).unwrap();
        let mut presenter = setup(&args, &mut io::sink()).unwrap();
        let assembled = presenter.assembled();
        let delivered = Rc::new(RefCell::new(String::new()));
        assembled
            .observation
            .subscribe(DeviceId::Mda, Box::new(Frames(delivered.clone())))
            .unwrap();
        assembly::run(
            &mut assembled.machine,
            &assembled.observation,
            &assembly::RunPolicy {
                retired_cap: Some(20_000_000),
                idle_limit: Some(u64::MAX),
                stop: assembly::StopCondition::PromptThen {
                    console: assembly::ConsoleProbe {
                        device: DeviceId::Mda,
                    },
                    needle: "Enter new date (mm-dd-[cc]yy):".into(),
                    poll_step: 1_000_000,
                    then_budget: 0,
                },
                stimuli: vec![],
            },
        )
        .unwrap();
        assert!(delivered
            .borrow()
            .contains("Enter new date (mm-dd-[cc]yy):"));
        // A redraw at the date prompt must keep running even before any key arrives.
        advance_frame(&mut presenter);
        assert!(presenter.running);
        for key in [
            KeyCode::Enter,
            KeyCode::Enter,
            KeyCode::KeyD,
            KeyCode::KeyI,
            KeyCode::KeyR,
            KeyCode::Enter,
        ] {
            presenter.key_event(key, Pressed);
            presenter.key_event(key, Released);
        }
        let start = presenter.assembled().machine.virtual_time();
        for _ in 0..400 {
            advance_frame(&mut presenter);
            assert!(presenter.running);
            if delivered.borrow().contains("2 file(s)")
                && presenter.keyboard().pending_events() == 0
            {
                break;
            }
        }
        let frame = delivered.borrow();
        for needle in [
            r"A:\>dir",
            r"Directory of A:\",
            "KERNEL",
            "COMMAND",
            "2 file(s)",
        ] {
            assert!(frame.contains(needle), "missing {needle:?}:\n{frame}");
        }
        drop(frame);
        assert_eq!(presenter.keyboard().pending_events(), 0);
        assert!(presenter.assembled().machine.virtual_time() > start);
        assert!(presenter.picture().is_some());
        assert!(presenter.window.is_none());
        let final_time = presenter.assembled().machine.virtual_time();
        let expected = guest_state(presenter.assembled());
        let mut replay = observed_machine(&config);
        replay.machine.set_idle_limit(Some(u64::MAX));
        replay_input(
            &mut replay,
            &presenter.machine().input_log,
            final_time,
            31_337,
        );
        assert_eq!(guest_state(&mut replay), expected);
    }

    /// Intel 80286 Programmer's Reference Manual, HLT; Intel 8254 datasheet,
    /// Mode 2: IRQ0 wakes the halted guest, whose handler acknowledges the PIC.
    fn timer_guest_config() -> assembly::MachineConfig {
        // IVT[8] -> F800:0100; PIC IRQ0 enabled; PIT mode 2, divisor 6000.
        #[rustfmt::skip]
        let program = [
            0xFA,
            0xC7, 0x06, 0x20, 0, 0, 1,
            0xC7, 0x06, 0x22, 0, 0, 0xF8,
            0xB0, 0x11, 0xE6, 0x20,
            0xB0, 8, 0xE6, 0x21,
            0xB0, 4, 0xE6, 0x21,
            0xB0, 1, 0xE6, 0x21,
            0xB0, 0xFE, 0xE6, 0x21,
            0xB0, 0x34, 0xE6, 0x43,
            0xB0, 0x70, 0xE6, 0x40,
            0xB0, 0x17, 0xE6, 0x40,
            0xFB, 0xF4, 0xEB, 0xFD,
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        rom[0x100..0x106].copy_from_slice(&[0x50, 0xB0, 0x20, 0xE6, 0x20, 0x58]);
        rom[0x106] = 0xCF;
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = assembly::SegmentSeed::flat(0);
        config
    }

    #[test]
    fn timer_waiting_guest_advances_across_presenter_chunks_without_a_window() {
        let config = timer_guest_config();
        let mut presenter = test_presenter(&config).unwrap();
        let mut previous_retired = 0;
        for chunk in 1..=3 {
            advance_frame(&mut presenter);
            assert!(presenter.running);
            let capture = presenter.assembled().capture_text(DeviceId::Mda).unwrap();
            assert_eq!(capture.virtual_time, chunk * TEST_FRAME_CLOCKS);
            assert!(
                capture.retired > previous_retired,
                "IRQ0 must wake the CPU in each chunk"
            );
            assert!(
                capture.retired < capture.virtual_time,
                "HLT must account for idle time"
            );
            previous_retired = capture.retired;
            assert!(presenter.picture().is_some());
        }
        assert!(presenter.window.is_none());
    }
}
