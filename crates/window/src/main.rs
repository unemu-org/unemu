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
use std::{
    error::Error,
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    time::Duration,
};
use ui_kit::{EGUI_FONT_NOTICE, SYMBOL_FONT_NOTICE};
#[cfg(test)]
use unemu_core_api::DisplayId;
use unemu_pc_core::{PcMachine, FONT_NOTICE};
#[cfg(test)]
use winit::{event::ElementState, keyboard::KeyCode};

/// The session's pacing, and the AT's tick rate for the tests that pace by it.
mod pacing {
    pub(crate) use machine_view::pacing::{wake, Speed};

    /// The AT's timer input, 14.31818 MHz divided by twelve (IBM Personal
    /// Computer AT Technical Reference, System Board, "System Timers"): the
    /// tick rate every PC describes.
    #[cfg(test)]
    pub(crate) const PIT: unemu_core_api::Rate = unemu_core_api::Rate::new(105_000_000, 88);

    /// Elapsed host time as ticks of the AT's timer input.
    #[cfg(test)]
    pub(crate) fn virtual_ticks(elapsed: std::time::Duration) -> u128 {
        machine_view::pacing::virtual_ticks(elapsed, PIT)
    }
}
pub(crate) use machine_view::system_keys;
// The PC media and library models, under the paths the desktop names them by.
use pc_library::library_worker;
#[cfg(test)]
use pc_library::pc_desktop;
use pc_media::{created_media, drive_chips, kept_catalogue};
// The PC media views, under the path the shell names them by.
use media_ui::catalogue_menu;
// The PC dialogs and Pull… from Remotes, under the paths the shell names them by.
use pc_ui::{remote_pull, settings_sheet};

mod app_menu;
mod diagnostics;
#[cfg(test)]
mod library_test_support;
mod library_view;
#[cfg(target_os = "macos")]
mod macos_menu;
mod media_dialog;
mod media_picker;
mod preferences;
mod preferences_sheet;
mod registries_model;
mod registry_transfer;
#[cfg(test)]
mod registry_transfer_view;
#[cfg(test)]
mod remote_browse;
mod settings_model;
mod shell;
#[cfg(test)]
mod shell_icons;
mod shell_model;

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
    /// The file this session's exception recording went to, once written;
    /// later writes replace it. See [`diagnostics`].
    recording_file: diagnostics::RecordingFile,
    /// Why the exception recording did not start, when it did not.
    recording_error: Option<String>,
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
        let assembled = machine_file::assemble_with_observation_and_options(
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
        Self::from_machine(
            PcMachine::from_assembled(config, assembled)?,
            display,
            warnings,
        )
    }

    /// Keep the PC catalogue alongside the contract session, using the core's
    /// effective configuration rather than a separate prepared machine.
    fn from_machine(
        machine: PcMachine,
        display: DeviceId,
        warnings: &mut impl io::Write,
    ) -> Result<Self, Box<dyn Error>> {
        use unemu_core_api::Machine;
        let config = machine.config().clone();
        let display_id = machine
            .description()
            .displays
            .iter()
            .find(|route| {
                route.label
                    == if display == DeviceId::Mda {
                        "MDA"
                    } else {
                        "VGA"
                    }
            })
            .map(|route| route.id)
            .ok_or("Machine has no display card")?;
        let mut media_slots = media_dialog::FloppySlot::from_config(&config);
        for (unit, slot) in media_slots.iter_mut().enumerate() {
            if let (Some(slot), Some(names)) = (
                slot,
                machine
                    .assembled_ref()
                    .floppy_names
                    .get(&(DeviceId::Fdc, unit as u8)),
            ) {
                for (entry, name) in slot.entries.iter_mut().zip(names) {
                    entry.medium.name = name.clone();
                }
            }
        }
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut session = Session::new(Box::new(machine), display_id, warnings)?;
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
            recording_file: diagnostics::RecordingFile::default(),
            recording_error: None,
        })
    }

    /// Logical asset keys cannot be used by host media tools or by the kept
    /// catalogue, which distinguishes an image's own disks by their host paths.
    fn with_catalogue_paths(mut self, paths: library_worker::CataloguePaths) -> Self {
        for (slot, entries) in paths.into_iter().enumerate() {
            let controller = if slot < 2 {
                DeviceId::Fdc
            } else {
                DeviceId::Ide
            };
            let unit = slot % 2;
            if let Some(slot) = self.media_slots.get_mut(slot).and_then(Option::as_mut) {
                for (entry, (image, write, name, format)) in slot.entries.iter_mut().zip(&entries) {
                    entry.medium.image = image.clone();
                    entry.medium.write = write.clone();
                    entry.medium.name = name.clone();
                    entry.geometry = *format;
                }
            }
            if controller == DeviceId::Fdc && !entries.is_empty() {
                self.session.machine_mut().assembled().floppy_names.insert(
                    (DeviceId::Fdc, unit as u8),
                    entries.iter().map(|(_, _, name, _)| name.clone()).collect(),
                );
            }
            if let Some(drive) = self
                .media_config
                .drives
                .iter_mut()
                .find(|drive| drive.controller == controller && usize::from(drive.unit) == unit)
            {
                if let Some(list) = &mut drive.media {
                    for (entry, (image, write, _, _)) in list.entries.iter_mut().zip(entries) {
                        entry.image = image;
                        entry.write = write;
                    }
                } else if let Some((image, write, _, _)) = entries.into_iter().next() {
                    drive.image = image;
                    drive.write = write;
                }
            }
        }
        self
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

    /// Start recording the exceptions and interrupts this session's guest
    /// takes; see [`diagnostics`]. A recording already running goes on.
    /// The report's deliveries start with it, if they had not yet.
    fn start_recording(&mut self) -> Result<(), String> {
        let machine = self.session.machine_mut();
        let started = machine
            .enable_delivery_log()
            .and_then(|()| machine.start_delivery_recording())
            .map_err(|error| error.to_string());
        self.recording_error = started.as_ref().err().cloned();
        started
    }

    /// Why the last start of a recording failed; `None` after one that
    /// worked.
    fn recording_error(&self) -> Option<&str> {
        self.recording_error.as_deref()
    }

    /// Whether a recording runs.
    fn recording(&self) -> bool {
        self.session.machine().delivery_recording_active()
    }

    /// Write the recording so far to its file in `folder`, the next
    /// `trace-N.txt` the first time; `None` while nothing records.
    fn write_recording(&mut self, folder: &Path) -> Option<Result<PathBuf, String>> {
        let text = self.session.machine().delivery_recording()?;
        Some(
            diagnostics::save_recording(folder, &mut self.recording_file, &text)
                .map_err(|error| error.to_string()),
        )
    }

    /// Write the recording to its file in `folder` and end it; `None`
    /// while nothing records. Without a folder the recording ends unwritten
    /// and the error says why.
    fn finish_recording(
        &mut self,
        folder: &Result<PathBuf, String>,
    ) -> Option<Result<PathBuf, String>> {
        let written = match folder {
            Ok(folder) => self.write_recording(folder),
            Err(error) => self.recording().then(|| Err(error.clone())),
        };
        self.session.machine_mut().stop_delivery_recording();
        self.recording_file = diagnostics::RecordingFile::default();
        written
    }

    fn write_reset_summary(&self, diagnostics: &mut impl io::Write) {
        if let Some(summary) = self.session.machine().reset_summary() {
            let _ = writeln!(diagnostics, "{summary}");
        }
    }
}

/// `--help`, `--version` or `--about` with the program's own list of cores.
#[cfg(test)]
fn informational_output(arg: &OsStr) -> Option<String> {
    informational_text(arg, &core_launch::Cores::new(platforms::cores()))
}

/// The text of `--help`, `--version` or `--about`; `--version` and `--about`
/// list each of `cores`.
fn informational_text(arg: &OsStr, cores: &core_launch::Cores) -> Option<String> {
    let opl3 = devices::OPL3_NOTICE;
    let shown = shown_path;
    if arg == "--help" {
        Some(format!(
            "usage: unemu [--machine FILE] [--open FILE [--core ID]] [--display CARD] [--rom-dir DIR]... [--allow-unlisted-firmware]\n\ndefault selection: {} (opens the machine library; --machine starts immediately)\nmachine files: {} (set {MACHINES_DIR_VAR} to use another directory)\n\n{FONT_NOTICE}\n\n{EGUI_FONT_NOTICE}\n\n{SYMBOL_FONT_NOTICE}\n\n{opl3}\n\n{}", shown(default_machine()), shown(machines_dir()), upcli::usage()
        ))
    } else if arg == "--version" || arg == "--about" {
        let mut lines = String::new();
        for line in cores.about_lines() {
            lines.push_str(&line);
            lines.push('\n');
        }
        Some(format!(
            "unemu {}\n{lines}{}\n\n{FONT_NOTICE}\n\n{EGUI_FONT_NOTICE}\n\n{SYMBOL_FONT_NOTICE}\n\n{opl3}",
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
/// the platform data directory, keeping the legacy tree while it exists. None when the host has
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
        .or_else(|| {
            data.and_then(|data| image_store::location::frontend::directory(&data, "machines").ok())
        })
}

fn default_machine() -> Option<PathBuf> {
    machines_dir().map(|directory| directory.join("at-freedos.toml"))
}

/// Where machines started from a file keep what they leave behind:
/// `unemu/saves` and `unemu/states` in the platform data directory,
/// retaining each legacy tree in place while it exists,
/// beside the machine files. `core_launch::start` creates a machine's own
/// directory under `saves` when the machine first starts; nothing is made
/// before. The reason is shown when Open… meets a host without a data
/// directory.
fn host_dirs() -> Result<core_launch::HostDirs, String> {
    image_store::location::data_directory()
        .map_err(|error| {
            format!("Files cannot be opened: the host has no data directory ({error})")
        })
        .and_then(|data| host_dirs_in(&data).map_err(|error| error.to_string()))
}

fn host_dirs_in(data: &Path) -> io::Result<core_launch::HostDirs> {
    Ok(core_launch::HostDirs {
        saves: image_store::location::frontend::directory(data, "saves")?,
        states: image_store::location::frontend::directory(data, "states")?,
    })
}

#[derive(Debug)]
struct Args {
    machine: PathBuf,
    /// `--open FILE`: a file to run at startup with whichever compiled-in
    /// core opens it, as File › Open… does.
    open: Option<PathBuf>,
    /// `--core ID`: the one core asked about the file of `--open`.
    core: Option<String>,
    rom_dirs: Vec<PathBuf>,
    display: Option<String>,
    options: assembly::AssemblyOptions,
    /// Print speaker and CPU-reset events to stderr; see [`SPEAKER_TRACE_VAR`].
    speaker_trace: bool,
    /// Where machine reports and exception recordings go, or why they
    /// cannot. See [`diagnostics`].
    diagnostics: Result<PathBuf, String>,
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
        open: None,
        core: None,
        rom_dirs: Vec::new(),
        display: None,
        options: assembly::AssemblyOptions::default(),
        speaker_trace: false,
        // `run` names the data directory's folder; tests name their own.
        diagnostics: Err("no diagnostics folder was named".into()),
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
            Some("--machine" | "--display" | "--open" | "--core") => {
                let value = args.next().filter(|value| !value.is_empty() && !value.as_encoded_bytes().starts_with(b"--"))
                    .ok_or("missing option value")?;
                if arg == "--machine" && !machine_seen {
                    result.machine = value.into();
                    machine_seen = true;
                } else if arg == "--display" && result.display.is_none() {
                    result.display = Some(value.into_string().map_err(|_| "invalid card id")?);
                } else if arg == "--open" && result.open.is_none() {
                    result.open = Some(value.into());
                } else if arg == "--core" && result.core.is_none() {
                    result.core = Some(value.into_string().map_err(|_| "invalid core id")?);
                } else {
                    return Err("duplicate option");
                }
            }
            _ => return Err("expected --machine FILE, --open FILE [--core ID], --display CARD or --allow-unlisted-firmware; positional paths are no longer supported"),
        }
    }
    // Each starts a machine, and the window runs one at a time.
    if machine_seen && result.open.is_some() {
        return Err("--machine and --open each start a machine; give one");
    }
    if result.core.is_some() && result.open.is_none() {
        return Err("--core names the core for --open FILE");
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
    let cores = core_launch::Cores::new(platforms::cores());
    if let [arg] = args.as_slice() {
        if let Some(output) = informational_text(arg, &cores) {
            println!("{output}");
            return Ok(());
        }
    }
    let mut args = parse_args(args)?;
    args.speaker_trace = speaker_trace_requested(std::env::var_os(SPEAKER_TRACE_VAR).as_deref());
    args.diagnostics = diagnostics::folder(image_store::location::data_directory());
    let autostart = std::env::args_os().skip(1).any(|arg| arg == "--machine");
    shell::run(args, autostart, cores)?;
    Ok(())
}

/// Commands start with a word; desktop options start with a dash. Unknown
/// words reach the command parser too, so it owns usage errors and exit codes.
fn command_word(first: Option<&OsStr>) -> bool {
    first.is_some_and(|arg| !arg.as_encoded_bytes().starts_with(b"-"))
}

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if command_word(args.first().map(|arg| arg.as_os_str())) {
        return std::process::ExitCode::from(upcli::run_command(&args));
    }
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
    use machine_file::assemble_with_observation;

    #[test]
    fn command_words_and_desktop_options_have_separate_routes() {
        assert!(!command_word(None));
        for option in [
            "--machine",
            "--display",
            "--open",
            "--version",
            "--about",
            "--help",
        ] {
            assert!(!command_word(Some(OsStr::new(option))), "{option}");
        }
        for word in ["image", "media", "recipe", "library", "run", "unknown"] {
            assert!(command_word(Some(OsStr::new(word))), "{word}");
        }
    }

    #[test]
    fn a_factory_presenter_keeps_the_selected_card_with_two_displays() {
        use cpu::Cpu as _;
        use sha2::Digest as _;
        use unemu_core_api::{Machine, RunLimit};
        let mut f = crate::library_test_support::Fixture::new();
        // Different letters make choosing the wrong observed card visible.
        let program = [
            0xfa, 0xb8, 0x00, 0xb0, 0x8e, 0xd8, 0xc7, 0x06, 0, 0, 0x41, 7, 0xb8, 0x00, 0xb8, 0x8e,
            0xd8, 0xc7, 0x06, 0, 0, 0x42, 7, 0xeb, 0xfe,
        ];
        let mut bios = vec![0xf4; 32768];
        bios[..program.len()].copy_from_slice(&program);
        bios[0x7ff0..0x7ff5].copy_from_slice(&[0xea, 0, 0, 0, 0xf8]);
        std::fs::write(f.temp.path().join("bios.bin"), bios).unwrap();
        f.row.descriptor = pc_image::test_support::with_synthetic_firmware(|| {
            pc_image::import_machine(
                &f.store,
                &f.temp.path().join("machine.toml"),
                "test:two-displays",
                &Default::default(),
            )
        })
        .unwrap()
        .manifest;
        let owner = f.store.workspace("two-displays").unwrap();
        let session = pc_image::test_support::with_synthetic_firmware(|| {
            let mut session =
                pc_image::workspace::prepare_pinned(&owner, &f.row.descriptor, true).unwrap();
            session
                .edit_launch(|launch| {
                    // Hardware order chooses VGA; the factory's display order starts with MDA.
                    let cards = launch.config.machine["card"].as_array_mut().unwrap();
                    let vga = cards
                        .iter()
                        .position(|card| card["kind"].as_str() == Some("vga-text"))
                        .unwrap();
                    cards.swap(0, vga);
                })
                .unwrap();
            session
        });
        let config = session.machine.clone();
        let direct = machine_file::assemble_with_verified_bases(
            &config,
            &ObservationPolicy {
                routes: vec![ObservationRequest {
                    device: DeviceId::Vga,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..Default::default()
            },
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &session.bases,
        )
        .unwrap();
        let mut before =
            Presenter::from_assembled(&config, direct, DeviceId::Vga, &mut io::sink()).unwrap();
        let stop = before.machine_mut().run(RunLimit::steps(100_000)).unwrap();
        let retired = before.assembled_ref().machine.cpu().retired();
        let expected = before
            .machine_mut()
            .frame(DisplayId(0))
            .unwrap()
            .pixels
            .to_vec();
        before.finish(&mut io::sink()).unwrap();
        drop(before);
        let prepared =
            pc_image::launch::from_session(session, &unemu_pc_core::PcCore, "Two displays".into())
                .unwrap();
        let machine = core_launch::start_with(
            &prepared
                .prepared
                .with_options(vec![("allow-unlisted-firmware".into(), "true".into())]),
            |r| unemu_pc_core::PcCore.create_machine(r),
        )
        .unwrap();
        assert_eq!(machine.description().displays.len(), 2);
        let mut presenter =
            Presenter::from_machine(machine, DeviceId::Vga, &mut io::sink()).unwrap();
        assert_eq!(presenter.session.display(), DisplayId(1));
        assert_eq!(
            presenter
                .machine_mut()
                .run(RunLimit::steps(100_000))
                .unwrap(),
            stop
        );
        assert_eq!(presenter.assembled_ref().machine.cpu().retired(), retired);
        let digest = |pixels: &[u32]| {
            sha2::Sha256::digest(
                pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
        };
        let vga_digest = digest(presenter.machine_mut().frame(DisplayId(1)).unwrap().pixels);
        assert_eq!(vga_digest, digest(&expected));
        assert_ne!(
            vga_digest,
            digest(presenter.machine_mut().frame(DisplayId(0)).unwrap().pixels)
        );
        eprintln!("VGA frame SHA-256: {vga_digest:x}");
        assert_eq!(
            presenter.machine_mut().frame(DisplayId(1)).unwrap().pixels,
            expected
        );
        presenter.finish(&mut io::sink()).unwrap();
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

    fn test_presenter(config: &assembly::MachineConfig) -> Result<Presenter, Box<dyn Error>> {
        Presenter::new(
            config,
            assembly::AssemblyOptions::default(),
            &mut io::sink(),
        )
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
        let temp = machine_file::test_support::TempDir::new("window-machine-display");
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
            open: None,
            core: None,
            rom_dirs: Vec::new(),
            display: None,
            options: assembly::AssemblyOptions::default(),
            speaker_trace: false,
            diagnostics: Err("none".into()),
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
        let temp = machine_file::test_support::TempDir::new("window-missing-default");
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
    fn desktop_binary_and_host_access_hint_name_unemu() {
        assert_eq!(env!("CARGO_BIN_NAME"), "unemu");
        assert!(system_keys::REFUSED_HINT.contains("allow unemu,"));
    }

    #[test]
    fn informational_options_carry_the_embedded_font_notice() {
        for option in ["--help", "--version", "--about"] {
            let output = informational_output(OsStr::new(option)).unwrap();
            assert!(output.contains(FONT_NOTICE), "{option}");
            assert!(output.starts_with("unemu ") || output.starts_with("usage: unemu "));
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

    /// `--version` and `--about` with the release's list of cores, pinned with
    /// the parts that change on their own (the release, the licence notice
    /// whose source line names the build, the third-party notices) replaced
    /// by names: the program's line, one line per compiled-in core, then the
    /// notices.
    #[test]
    fn version_and_about_are_pinned() {
        for option in ["--version", "--about"] {
            let text = informational_output(OsStr::new(option))
                .unwrap()
                .replace(&platforms::licence_notice(), "LICENCE NOTICE")
                .replace(FONT_NOTICE, "FONT NOTICE")
                .replace(EGUI_FONT_NOTICE, "EGUI FONT NOTICE")
                .replace(SYMBOL_FONT_NOTICE, "SYMBOL FONT NOTICE")
                .replace(devices::OPL3_NOTICE, "OPL3 NOTICE")
                .replace(env!("CARGO_PKG_VERSION"), "VERSION");
            assert_eq!(
                text,
                "unemu VERSION\n\
                 core unemupc VERSION (unemupc), contract 0.3\n\
                 core unemunes 0.0.1 (unemunes), contract 0.3\n\
                 LICENCE NOTICE\n\nFONT NOTICE\n\nEGUI FONT NOTICE\n\n\
                 SYMBOL FONT NOTICE\n\nOPL3 NOTICE",
                "{option}"
            );
        }
    }

    /// `--help` whole, with the default paths and the notices replaced by
    /// names: the usage line, the defaults, then the notices.
    #[test]
    fn help_is_pinned() {
        let text = informational_output(OsStr::new("--help"))
            .unwrap()
            .replace(&shown_path(default_machine()), "DEFAULT")
            .replace(&shown_path(machines_dir()), "MACHINES")
            .replace(FONT_NOTICE, "FONT NOTICE")
            .replace(EGUI_FONT_NOTICE, "EGUI FONT NOTICE")
            .replace(SYMBOL_FONT_NOTICE, "SYMBOL FONT NOTICE")
            .replace(devices::OPL3_NOTICE, "OPL3 NOTICE")
            .replace(&upcli::usage(), "COMMAND USAGE");
        assert_eq!(
            text,
            "usage: unemu [--machine FILE] [--open FILE [--core ID]] [--display CARD] [--rom-dir DIR]... [--allow-unlisted-firmware]\n\n\
             default selection: DEFAULT (opens the machine library; --machine starts immediately)\n\
             machine files: MACHINES (set UNEMUPC_MACHINES_DIR to use another directory)\n\n\
             FONT NOTICE\n\nEGUI FONT NOTICE\n\nSYMBOL FONT NOTICE\n\nOPL3 NOTICE\n\nCOMMAND USAGE"
        );
    }

    #[test]
    fn open_takes_a_file_and_names_a_core_only_with_it() {
        let parse = |args: &[&str]| parse_args(args.iter().map(Into::into));
        let args = parse(&["--open", "game.utc"]).unwrap();
        assert_eq!(args.open, Some(PathBuf::from("game.utc")));
        assert_eq!(args.core, None);
        let args = parse(&["--core", "unemu-test-core", "--open", "game.utc"]).unwrap();
        assert_eq!(args.open, Some(PathBuf::from("game.utc")));
        assert_eq!(args.core.as_deref(), Some("unemu-test-core"));
        assert_eq!(
            parse(&["--core", "unemu-test-core"]).unwrap_err(),
            "--core names the core for --open FILE"
        );
        assert_eq!(
            parse(&["--machine", "machine.toml", "--open", "game.utc"]).unwrap_err(),
            "--machine and --open each start a machine; give one"
        );
        for args in [
            &["--open"][..],
            &["--open", "--core", "x"],
            &["--open", "a.utc", "--open", "b.utc"],
            &["--open", "a.utc", "--core", "x", "--core", "y"],
            &["--open", "a.utc", "--core"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    fn legacy_machine(data: &Path, journal: &Path) -> PathBuf {
        let directory = data.join("unemupc/machines/target");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("bios.bin"), vec![0xf4; 0x8000]).unwrap();
        std::fs::write(directory.join("base.img"), vec![0; 368_640]).unwrap();
        let machine = directory.join("machine.toml");
        let journal = toml::Value::String(journal.to_string_lossy().into_owned());
        std::fs::write(&machine, format!(
            "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='base.img'\nwrite='journal'\njournal={journal}\n"
        )).unwrap();
        machine
    }

    #[test]
    fn an_absolute_legacy_journal_remains_the_only_journal() {
        use journal_files::{JournalMedia, SectorSource, SinkStatus, VerifiedBase};
        let temp = machine_file::test_support::TempDir::new("legacy-absolute-journal");
        let data = temp.path().canonicalize().unwrap();
        let journal = data.join("unemupc/machines/target/disk.upfj");
        legacy_machine(&data, &journal);
        let base_path = data.join("unemupc/machines/target/base.img");
        let base_bytes = std::fs::read(&base_path).unwrap();
        let base = VerifiedBase::open(&base_path).unwrap();
        let open = |path: &Path| {
            JournalMedia::open_verified(path, &base, base.byte_len(), SinkStatus::default())
                .unwrap()
        };
        {
            let mut disk = open(&journal);
            disk.write_batch(&[(0, [1; 512])]);
            disk.checkpoint().unwrap();
        }
        let selected = image_store::location::frontend::directory(&data, "machines").unwrap();
        for value in [2, 3] {
            let config = machine_file::load(selected.join("target/machine.toml")).unwrap();
            let assembly::WriteMode::Journal { path, .. } = &config.drives[0].write else {
                panic!("expected journal persistence");
            };
            assert_eq!(path, &journal);
            {
                let mut disk = open(path);
                disk.write_batch(&[(0, [value; 512])]);
                disk.checkpoint().unwrap();
            }
            let disk = open(&selected.join("target/disk.upfj"));
            assert_eq!(
                disk.read_sector(0),
                [value; 512],
                "the selected tree must see the same journal"
            );
        }
        assert!(!data.join("unemu/machines").exists());
        assert_eq!(std::fs::read(base_path).unwrap(), base_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_legacy_machine_keeps_target_relative_paths() {
        let temp = machine_file::test_support::TempDir::new("legacy-machine-link");
        let data = temp.path().canonicalize().unwrap();
        let target = legacy_machine(&data, Path::new("disk.upfj"));
        let old = data.join("unemupc/machines");
        std::os::unix::fs::symlink("target/machine.toml", old.join("alias.toml")).unwrap();
        let selected = image_store::location::frontend::directory(&data, "machines").unwrap();
        let config = machine_file::load(selected.join("alias.toml")).unwrap();
        assert_eq!(
            config.drives[0].write,
            assembly::WriteMode::journal(target.parent().unwrap().join("disk.upfj"))
        );
        assert!(std::fs::symlink_metadata(selected.join("alias.toml"))
            .unwrap()
            .is_symlink());
        assert!(!data.join("unemu/machines").exists());
    }

    #[test]
    fn files_opened_keep_their_saves_and_states_in_the_data_directory() {
        let temp = machine_file::test_support::TempDir::new("host-dirs");
        for name in ["saves", "states"] {
            let old = temp
                .path()
                .join("unemupc")
                .join(name)
                .join("core/aa/content");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::write(old.join("record"), name).unwrap();
        }
        let dirs = host_dirs_in(temp.path()).unwrap();
        assert_eq!(dirs.saves, temp.path().join("unemupc/saves"));
        assert_eq!(dirs.states, temp.path().join("unemupc/states"));
        for (name, path) in [("saves", dirs.saves), ("states", dirs.states)] {
            let record = path.join("core/aa/content/record");
            assert_eq!(std::fs::read_to_string(&record).unwrap(), name);
            std::fs::write(&record, "new").unwrap();
            assert_eq!(
                std::fs::read_to_string(
                    temp.path()
                        .join("unemupc")
                        .join(name)
                        .join("core/aa/content/record")
                )
                .unwrap(),
                "new"
            );
        }
    }

    #[test]
    fn machine_files_come_from_the_switch_then_cargo_then_the_platform_data_directory() {
        let data = || Some(PathBuf::from("/data/user"));
        let crate_dir = || Some(std::ffi::OsString::from("/src/tree/crates/window"));
        let from_cargo = PathBuf::from("/src/tree/crates/window/../../machines");
        let from_data = PathBuf::from("/data/user/unemu/machines");
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
}
