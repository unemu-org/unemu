// SPDX-License-Identifier: AGPL-3.0-or-later
//! Minimal FreeDOS window frontend (design/spec.md §9.4).
//!
//! The event loop alone reads a monotonic host clock. A pure pacing policy
//! converts elapsed time to bounded PIT targets and virtual-time run caps.
//! Input enters only at run boundaries; observation captures the completed
//! boundary. See `backlog/docs/design/observation.md` § Window pacing.

use assembly::{AssembledMachine, DeviceId, ObservationPolicy, ObservationRequest, OutputKind};
use machine::RunOutcome;
use softbuffer::{Context, Surface};
use std::{
    error::Error,
    ffi::OsStr,
    io,
    num::{NonZeroU16, NonZeroU32, NonZeroU64},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};
use window::{HostKeyboard, WindowSink, FONT_NOTICE};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

mod pacing;

use pacing::{frame_target, next_frame, pit_clocks, run_cap, wake, CpuRate};
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

type HostSurface = Surface<Rc<Window>, Rc<Window>>;

struct Presenter {
    assembled: AssembledMachine,
    display: DeviceId,
    sink: WindowSink,
    surface: Option<HostSurface>,
    window: Option<Rc<Window>>,
    running: bool,
    reset_requests: u64,
    maintenance_reported: bool,
    keyboard: HostKeyboard,
    error: Option<Box<dyn Error>>,
    rate: CpuRate,
    dropped_clocks: u128,
    started: Option<Instant>,
    next_paint: Option<Duration>,
    occluded_since: Option<Duration>,
    #[cfg(test)]
    input_log: Vec<(u64, Vec<u8>)>,
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

    fn from_assembled(
        config: &assembly::MachineConfig,
        mut assembled: AssembledMachine,
        display: DeviceId,
        warnings: &mut impl io::Write,
    ) -> Result<Self, Box<dyn Error>> {
        for unlisted in &assembled.unlisted_firmware {
            writeln!(warnings, "{unlisted}")?;
        }
        let clock = &config.clock;
        // HLT must wait for an IRQ; the pacing cap already bounds idle time.
        assembled.machine.set_idle_limit(Some(u64::MAX));
        let sink = WindowSink::new(NonZeroU16::new(2).ok_or("invalid scale")?)?;
        assembled
            .observation
            .subscribe(display, Box::new(sink.clone()))?;
        Ok(Self {
            assembled,
            display,
            sink,
            surface: None,
            window: None,
            running: true,
            reset_requests: 0,
            maintenance_reported: false,
            keyboard: HostKeyboard::default(),
            error: None,
            rate: CpuRate {
                numerator: NonZeroU64::new(clock.retired_per_pit_clock).ok_or("zero CPU rate")?,
                denominator: NonZeroU64::new(clock.retired_per_pit_clock_denominator)
                    .ok_or("zero CPU rate denominator")?,
            },
            dropped_clocks: 0,
            started: None,
            next_paint: None,
            occluded_since: None,
            #[cfg(test)]
            input_log: Vec::new(),
        })
    }

    fn open(&mut self, events: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        let window = Rc::new(
            events.create_window(
                Window::default_attributes()
                    .with_title("unemupc98 — freedos")
                    .with_inner_size(PhysicalSize::new(1440, 700)),
            )?,
        );
        let context = Context::new(window.clone())?;
        self.surface = Some(Surface::new(&context, window.clone())?);
        window.request_redraw();
        self.window = Some(window);
        Ok(())
    }

    fn advance(&mut self, elapsed: Duration) -> Result<(), Box<dyn Error>> {
        self.advance_with_warnings(elapsed, &mut io::stderr())
    }

    fn advance_with_warnings(
        &mut self,
        elapsed: Duration,
        warnings: &mut impl io::Write,
    ) -> Result<(), Box<dyn Error>> {
        if !self.running || self.occluded_since.is_some() {
            return Ok(());
        }
        self.assembled.check_persistence()?;
        let (target, dropped) = frame_target(
            elapsed,
            self.assembled.machine.clock().0,
            self.dropped_clocks,
        );
        self.dropped_clocks = dropped;
        while self.running && self.assembled.machine.clock().0 < target {
            let time = self.assembled.machine.virtual_time();
            let cap = run_cap(target, self.assembled.machine.clock().0, time, self.rate);
            if cap == time {
                self.running = false;
                break;
            }
            self.assembled.check_persistence()?;
            let delivered = self.keyboard.inject_at_boundary(&mut self.assembled);
            #[cfg(test)]
            if !delivered.is_empty() {
                self.input_log.push((time, delivered));
            }
            #[cfg(not(test))]
            drop(delivered);
            let outcome = self.assembled.machine.run_bounded(Some(cap));
            if matches!(outcome, Ok(RunOutcome::ResetRequested)) {
                self.record_reset_request(&mut io::stderr());
            }
            self.assembled.check_persistence()?;
            match outcome {
                Ok(RunOutcome::RetiredCapReached) => self.running = cap < u64::MAX,
                Ok(RunOutcome::ResetRequested) => {}
                Ok(RunOutcome::Halted) => {
                    self.running = false;
                    eprintln!("machine halted");
                }
                Err(error) => {
                    self.running = false;
                    eprintln!("machine stopped: {error}");
                    self.error = Some(error.into());
                }
            }
        }
        if !self.running && self.error.is_none() {
            self.finish_persistence(warnings)?;
        }
        // Even a CPU error must leave the screen at its last completed boundary.
        self.assembled.capture_text(self.display)?;
        if let Some(error) = self.sink.error() {
            return Err(error.into());
        }
        Ok(())
    }

    fn finish_persistence(
        &mut self,
        warnings: &mut impl io::Write,
    ) -> Result<(), assembly::PersistenceError> {
        let result = self.assembled.finish_persistence();
        if !self.maintenance_reported {
            for handle in &self.assembled.persistence {
                if let Some(outcome) = handle.maintenance_failure() {
                    let _ = writeln!(warnings, "warning: drive {}: {outcome}", handle.drive_index);
                }
            }
            self.maintenance_reported = true;
        }
        result
    }

    fn record_reset_request(&mut self, diagnostics: &mut impl io::Write) {
        self.reset_requests = self.reset_requests.saturating_add(1);
        if self.reset_requests == 1 {
            let _ = writeln!(
                diagnostics,
                "cpu reset requested; continuing to guest fallback"
            );
        }
    }

    fn write_reset_summary(&self, diagnostics: &mut impl io::Write) {
        if self.reset_requests != 0 {
            let _ = writeln!(
                diagnostics,
                "cpu reset requests in session: {}",
                self.reset_requests
            );
        }
    }

    fn occlusion_changed(&mut self, occluded: bool, elapsed: Duration) {
        if occluded {
            self.occluded_since.get_or_insert(elapsed);
            self.next_paint = None;
        } else if let Some(since) = self.occluded_since.take() {
            // Hidden time grants no guest work when presentation resumes.
            self.dropped_clocks += pit_clocks(elapsed) - pit_clocks(since);
            self.next_paint = Some(elapsed);
        }
    }

    fn key_event(&mut self, key: KeyCode, state: ElementState) {
        if self.running && self.error.is_none() {
            self.keyboard.key_event(key, state);
        }
    }

    fn focus_changed(&mut self, focused: bool) {
        if !focused && self.running && self.error.is_none() {
            self.keyboard.release_all();
        }
    }

    fn paint(&mut self) -> Result<(), Box<dyn Error>> {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        surface.resize(width, height)?;
        let mut buffer = surface.buffer_mut()?;
        buffer.fill(0);
        if let Some(frame) = self.sink.frame().as_ref() {
            let left = size.width.saturating_sub(frame.width) / 2;
            let top = size.height.saturating_sub(frame.height) / 2;
            let copy_width = frame.width.min(size.width) as usize;
            for y in 0..frame.height.min(size.height) {
                let src = (y * frame.width) as usize;
                let dst = ((y + top) * size.width + left) as usize;
                buffer[dst..dst + copy_width].copy_from_slice(&frame.pixels[src..src + copy_width]);
            }
        }
        window.pre_present_notify();
        buffer.present()?;
        Ok(())
    }

    fn fail(&mut self, events: &ActiveEventLoop, error: Box<dyn Error>) {
        self.running = false;
        self.error = Some(error);
        events.exit();
    }
}

impl ApplicationHandler for Presenter {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(error) = self.open(events) {
                self.fail(events, error);
                return;
            }
            let now = Instant::now();
            let started = *self.started.get_or_insert(now);
            self.occlusion_changed(false, now - started);
            self.next_paint = Some(now - started);
        }
    }

    fn suspended(&mut self, _events: &ActiveEventLoop) {
        if let Some(started) = self.started {
            self.occlusion_changed(true, started.elapsed());
        }
        self.surface = None;
        self.window = None;
        self.next_paint = None;
    }

    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => events.exit(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key) = event.physical_key {
                    self.key_event(key, event.state);
                }
            }
            WindowEvent::Focused(focused) => self.focus_changed(focused),
            WindowEvent::Occluded(occluded) => {
                if let Some(started) = self.started {
                    self.occlusion_changed(occluded, started.elapsed());
                }
                if occluded {
                    events.set_control_flow(ControlFlow::Wait);
                }
            }
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // Some platforms still deliver queued redraws while occluded.
                if self.occluded_since.is_some() {
                    return;
                }
                if let Some(started) = self.started {
                    let elapsed = started.elapsed();
                    if let Err(error) = self.advance(elapsed).and_then(|()| self.paint()) {
                        self.fail(events, error);
                    }
                    self.next_paint = Some(next_frame(started.elapsed()));
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        if let (Some(window), Some(started)) = (&self.window, self.started) {
            let (redraw, deadline) = wake(
                started.elapsed(),
                self.next_paint,
                self.running && self.occluded_since.is_none(),
            );
            self.next_paint = deadline;
            if redraw {
                window.request_redraw();
            }
            if let Some(deadline) = deadline {
                events.set_control_flow(ControlFlow::WaitUntil(started + deadline));
                return;
            }
        }
        events.set_control_flow(ControlFlow::Wait);
    }
}

fn informational_output(arg: &OsStr) -> Option<String> {
    if arg == "--help" {
        Some(format!(
            "usage: unemupc98-window [--machine FILE] [--display CARD] [--allow-unlisted-firmware]\n\ndefault machine: {}\n\n{FONT_NOTICE}", default_machine().display()
        ))
    } else if arg == "--version" || arg == "--about" {
        Some(format!(
            "unemupc98-window {}\n\n{FONT_NOTICE}",
            env!("CARGO_PKG_VERSION")
        ))
    } else {
        None
    }
}

fn default_machine() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml")
}

#[derive(Debug)]
struct Args {
    machine: PathBuf,
    display: Option<String>,
    options: assembly::AssemblyOptions,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Args, &'static str> {
    let mut result = Args {
        machine: default_machine(),
        display: None,
        options: assembly::AssemblyOptions::default(),
    };
    let mut machine_seen = false;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--allow-unlisted-firmware") => result.options.allow_unlisted_firmware = true,
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

fn load_machine(
    path: &Path,
    default: &Path,
    options: assembly::AssemblyOptions,
) -> Result<machine_file::LoadedMachine, Box<dyn Error>> {
    machine_file::load_for_assembly(path, options).map_err(|error| {
        if path == default && !path.exists() {
            format!("{error}; use --machine FILE to select a machine description")
        } else {
            error.to_string()
        }
        .into()
    })
}

fn setup(args: &Args, warnings: &mut impl io::Write) -> Result<Presenter, Box<dyn Error>> {
    let loaded = load_machine(&args.machine, &default_machine(), args.options)?;
    let display = selected_display(&loaded, args.display.as_deref())?;
    let assembled = loaded.assemble(&ObservationPolicy {
        routes: vec![ObservationRequest {
            device: display,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
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
    let args = parse_args(args)?;
    let mut presenter = setup(&args, &mut io::stderr())?;
    let events = EventLoop::new()?;
    events.set_control_flow(ControlFlow::Wait);
    let result = events.run_app(&mut presenter);
    presenter.write_reset_summary(&mut io::stderr());
    result?;
    // A halt already finished its session in advance; closing a running window
    // is the other normal end of a session.
    if presenter.running && presenter.error.is_none() {
        presenter.finish_persistence(&mut io::stderr())?;
    }
    if let Some(error) = presenter.error {
        return Err(error);
    }
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

    fn test_presenter(config: &assembly::MachineConfig) -> Result<Presenter, Box<dyn Error>> {
        Presenter::new(
            config,
            assembly::AssemblyOptions::default(),
            &mut io::sink(),
        )
    }

    #[test]
    fn machine_file_setup_equals_the_preset_without_opening_a_window() {
        let required = std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES");
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
        let loaded = machine_file::load_for_assembly(
            default_machine(),
            assembly::AssemblyOptions::default(),
        )
        .unwrap();
        assert_eq!(loaded.config, assembly::bios(rom, Some(floppy)));
        let args = parse_args([]).unwrap();
        assert!(args.machine.is_absolute());
        assert_eq!(
            args.machine,
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../machines/at-freedos.toml")
        );
        let presenter = setup(&args, &mut io::sink()).unwrap();
        assert_eq!(presenter.display, DeviceId::Mda);
        assert!(presenter.window.is_none());
        assert_eq!(presenter.assembled.machine.virtual_time(), 0);
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
            assert_eq!(presenter.assembled.observation.routes()[0].device, expected);
            assert!(presenter.assembled.capture_text(expected).is_ok());
            assert!(presenter.window.is_none());
        }
        assert!(setup(&args_for(Some("missing")), &mut io::sink()).is_err());
        let missing = Args {
            machine: temp.path().join("missing.toml"),
            display: None,
            options: assembly::AssemblyOptions::default(),
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
        assert!(help.contains(&format!("default machine: {}", default_machine().display())));
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
        assert_eq!(presenter.assembled.unlisted_firmware.len(), 1);
        assert_eq!(String::from_utf8(warnings).unwrap(), "warning: running unlisted firmware (sha256 c35020473aed1b4642cd726cad727b63fff2824ad68cedd7ffb73c7cbd890479)\n");
    }

    #[test]
    fn journal_host_failure_stops_at_the_first_window_boundary() {
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
        config.drives[0].image = assembly::ImageSource::File(base);
        let mut reference = test_presenter(&config).unwrap();
        assert_eq!(
            reference
                .assembled
                .machine
                .run_bounded(Some(10_000))
                .unwrap(),
            RunOutcome::ResetRequested
        );
        let boundary = reference.assembled.machine.virtual_time();
        config.drives[0].write = assembly::WriteMode::journal(&journal);
        let mut presenter = test_presenter(&config).unwrap();
        presenter.assembled.persistence[0].fail_output();
        let error = presenter.advance(Duration::from_millis(20)).unwrap_err();
        assert!(error.to_string().contains("drive 0 persistence"), "{error}");
        assert_eq!(presenter.assembled.machine.virtual_time(), boundary);
        assert_eq!(presenter.reset_requests, 1);
        assert!(presenter.advance(Duration::from_millis(40)).is_err());
        assert_eq!(presenter.assembled.machine.virtual_time(), boundary);
        assert_eq!(std::fs::read(journal).unwrap().len(), 128);
        assert!(presenter.window.is_none());
    }

    #[test]
    fn journal_presenter_finishes_halts_and_preserves_unsealed_execution_errors() {
        for fail in [false, true] {
            let temp = assembly::test_support::TempDir::new("journal-test");
            let path = temp.path();
            let base = path.join("base.img");
            let journal = path.join("disk.upfj");
            std::fs::write(&base, vec![0; 1_474_560]).unwrap();
            let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
            config.board = assembly::BoardModel::Bare;
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
            presenter.advance(Duration::from_millis(20)).unwrap();
            assert!(!presenter.running);
            assert_eq!(presenter.error.is_some(), fail);
            let bytes = std::fs::read(journal).unwrap();
            assert_eq!(bytes.len(), if fail { 128 } else { 128 + 640 });
            if !fail {
                assert_eq!(bytes[128 + 8], 3);
            }
            drop(presenter);
        }
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
                for handle in &mut presenter.assembled.persistence {
                    handle.checkpoint().unwrap();
                }
                presenter.assembled.persistence[0].fail_retention(operation);
                if both_fail {
                    presenter.assembled.persistence[1].fail_retention("temporary write");
                }
                let mut warnings = Vec::new();
                presenter
                    .advance_with_warnings(Duration::from_millis(20), &mut warnings)
                    .unwrap();
                assert!(!presenter.running);
                assert!(presenter.error.is_none());
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
        let target = presenter.assembled.machine.clock().0 + TEST_FRAME_CLOCKS;
        let nanos = (u128::from(target) * 12_000_000_000).div_ceil(14_318_180);
        presenter
            .advance(Duration::from_nanos(nanos as u64))
            .unwrap();
    }

    #[test]
    fn informational_options_carry_the_embedded_font_notice() {
        for option in ["--help", "--version", "--about"] {
            let output = informational_output(OsStr::new(option)).unwrap();
            assert!(output.contains(FONT_NOTICE), "{option}");
            assert!(output.contains("unemupc98-window"));
        }
        assert!(informational_output(OsStr::new("bios.bin")).is_none());
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
        assert_eq!(presenter.keyboard.pending_events(), 0);
        presenter.running = true;
        presenter.error = Some("presentation failed".into());
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Pressed);
        assert_eq!(presenter.keyboard.pending_events(), 0);
        presenter.error = None;
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Pressed);
        presenter.focus_changed(true);
        assert_eq!(presenter.keyboard.pending_events(), 1);
        presenter.focus_changed(false);
        assert_eq!(presenter.keyboard.pending_events(), 2);
        presenter.key_event(KeyCode::ShiftLeft, ElementState::Released);
        assert_eq!(presenter.keyboard.pending_events(), 2);
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
            .assembled
            .capture_text(DeviceId::Mda)
            .unwrap()
            .frame;
        assert!(frame
            .to_dump()
            .starts_with(std::str::from_utf8(prompt).unwrap()));
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        advance_frame(&mut presenter);
        assert!(presenter.running);
        assert_eq!(
            presenter.assembled.machine.virtual_time(),
            3 * TEST_FRAME_CLOCKS
        );
        let video::Frame::Text(frame) = presenter
            .assembled
            .capture_text(DeviceId::Mda)
            .unwrap()
            .frame;
        assert!(frame.to_dump().contains("Enter new date (mm-dd-[cc]yy):!"));
        assert!(presenter.sink.frame().is_some());
        assert!(presenter.window.is_none());
    }

    #[test]
    fn reset_diagnostics_log_only_the_first_request_and_the_session_total() {
        let config = assembly::bios(vec![0; 0x8000], None);
        let mut presenter = test_presenter(&config).unwrap();
        let mut diagnostics = Vec::new();
        presenter.write_reset_summary(&mut diagnostics);
        assert!(diagnostics.is_empty());
        for _ in 0..10_000 {
            presenter.record_reset_request(&mut diagnostics);
        }
        assert_eq!(
            diagnostics,
            b"cpu reset requested; continuing to guest fallback\n"
        );
        assert_eq!(presenter.reset_requests, 10_000);
        presenter.write_reset_summary(&mut diagnostics);
        assert_eq!(diagnostics, b"cpu reset requested; continuing to guest fallback\ncpu reset requests in session: 10000\n");
        assert!(presenter.window.is_none());
    }

    #[test]
    fn a_guest_reset_loop_counts_requests_and_keeps_running() {
        let mut rom = vec![0; 0x8000];
        rom[..6].copy_from_slice(&[0xB0, 0xFE, 0xE6, 0x64, 0xEB, 0xFC]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        advance_frame(&mut presenter);
        assert!(presenter.reset_requests > 1_000);
        assert!(presenter.running);
        assert_eq!(
            presenter.assembled.machine.virtual_time(),
            TEST_FRAME_CLOCKS
        );
        let count = presenter.reset_requests;
        advance_frame(&mut presenter);
        assert!(presenter.reset_requests > count);
        let mut summary = Vec::new();
        presenter.write_reset_summary(&mut summary);
        assert_eq!(
            summary,
            format!(
                "cpu reset requests in session: {}\n",
                presenter.reset_requests
            )
            .as_bytes()
        );
        assert!(presenter.window.is_none());
    }

    #[test]
    fn window_continues_to_the_fallback_boot_jump_after_a_reset_request() {
        use cpu::{Cpu, CpuReg};
        let mut rom = vec![0; 0x8000];
        rom[..9].copy_from_slice(&[0xB0, 0xFE, 0xE6, 0x64, 0xEA, 0, 0, 0xFF, 0xFF]);
        rom[0x7FF0..0x7FF4].copy_from_slice(&[0xB0, 0x55, 0xFA, 0xF4]);
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        advance_frame(&mut presenter);
        assert_eq!(
            presenter.assembled.machine.cpu().reg(CpuReg::Rax) & 0xFF,
            0x55
        );
        assert_eq!(presenter.assembled.machine.virtual_time(), 6);
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
        presenter.keyboard.key_event(KeyCode::KeyF, Pressed);
        presenter.keyboard.key_event(KeyCode::KeyG, Pressed);
        assert_eq!(presenter.keyboard.pending_events(), 2);
        assert_eq!(presenter.assembled.machine.virtual_time(), 0);
        assert_eq!(
            presenter
                .assembled
                .machine
                .bus_mut()
                .port_read(0x64, 1)
                .unwrap()
                & 1,
            0
        );
        advance_frame(&mut presenter);
        assert_eq!(presenter.keyboard.pending_events(), 1);
        let first = presenter.assembled.capture_text(DeviceId::Mda).unwrap();
        let video::Frame::Text(first) = first.frame;
        assert!(first.to_dump().starts_with("1!"));
        assert!(
            !first.to_dump().starts_with("1!\""),
            "second byte must wait for another boundary"
        );
        advance_frame(&mut presenter);
        assert_eq!(presenter.keyboard.pending_events(), 0);
        let second = presenter.assembled.capture_text(DeviceId::Mda).unwrap();
        let video::Frame::Text(second) = second.frame;
        assert!(second.to_dump().starts_with("1!\""));
        assert_eq!(
            presenter.assembled.machine.virtual_time(),
            2 * TEST_FRAME_CLOCKS
        );
        assert!(presenter.sink.frame().is_some());
        assert!(presenter.window.is_none());
    }

    fn replay_input(
        assembled: &mut AssembledMachine,
        log: &[(u64, Vec<u8>)],
        final_time: u64,
        chunk: u64,
    ) {
        let keyboard = assembled.device_index[&DeviceId::Keyboard];
        for (time, bytes) in log {
            run_to(assembled, *time, chunk);
            for byte in bytes {
                assert!(assembly::try_inject_scancode(
                    &mut assembled.machine,
                    keyboard,
                    *byte
                ));
            }
        }
        run_to(assembled, final_time, chunk);
    }

    fn run_to(assembled: &mut AssembledMachine, time: u64, chunk: u64) {
        while assembled.machine.virtual_time() < time {
            let cap = (assembled.machine.virtual_time() + chunk).min(time);
            assert_eq!(
                assembled.machine.run_bounded(Some(cap)).unwrap(),
                RunOutcome::RetiredCapReached
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
                paced.advance(Duration::from_millis(millis)).unwrap();
                assert!(paced.running);
            }
            assert!(paced.dropped_clocks > 0, "the fake stall must drop backlog");
            assert_eq!(paced.keyboard.pending_events(), 0);
            let bytes: Vec<_> = paced
                .input_log
                .iter()
                .flat_map(|(_, bytes)| bytes.clone())
                .collect();
            assert_eq!(bytes, [0x21, 0x22, 0xA1, 0xA2, 0xE0, 0x4D, 0xE0, 0xCD]);
            assert_eq!(paced.input_log[0].0, 0);
            let final_time = paced.assembled.machine.virtual_time();
            let expected = guest_state(&mut paced.assembled);
            for chunk in [137, 997] {
                let mut headless = observed_machine(&config);
                replay_input(&mut headless, &paced.input_log, final_time, chunk);
                assert_eq!(
                    guest_state(&mut headless),
                    expected,
                    "rate {a}/{b}, chunk {chunk}"
                );
            }
            // Prove that this guest/state oracle is sensitive to timing, rather
            // than merely checking that the same byte string eventually arrives.
            let shifted: Vec<_> = paced
                .input_log
                .iter()
                .map(|(time, bytes)| (time + 17, bytes.clone()))
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
                let (target, _) = frame_target(
                    elapsed,
                    presenter.assembled.machine.clock().0,
                    presenter.dropped_clocks,
                );
                presenter.advance(elapsed).unwrap();
                let clock = presenter.assembled.machine.clock().0;
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
        presenter.advance(visible).unwrap();
        let before = guest_state(&mut presenter.assembled);
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        presenter.occlusion_changed(true, visible);
        // Repeated notifications cannot shorten the hidden interval.
        presenter.occlusion_changed(true, Duration::from_secs(1));
        let resumed = Duration::from_secs(120);
        presenter.advance(resumed).unwrap();
        assert_eq!(guest_state(&mut presenter.assembled), before);
        assert_eq!(presenter.keyboard.pending_events(), 1);
        assert!(presenter.input_log.is_empty());
        assert_eq!(
            wake(
                resumed,
                presenter.next_paint,
                presenter.occluded_since.is_none()
            ),
            (false, None)
        );
        presenter.occlusion_changed(false, resumed);
        assert_eq!(
            presenter.dropped_clocks,
            pit_clocks(resumed) - pit_clocks(visible)
        );
        assert!(wake(resumed, presenter.next_paint, true).0);
        presenter.advance(resumed).unwrap();
        assert_eq!(guest_state(&mut presenter.assembled), before);
        presenter
            .advance(resumed + Duration::from_millis(10))
            .unwrap();
        assert!(presenter.assembled.machine.clock().0 > before.clock);
        assert_eq!(presenter.keyboard.pending_events(), 0);
    }

    #[test]
    fn identical_host_time_does_not_advance_or_deliver_queued_input() {
        let mut config = assembly::bios(vec![0x90; 0x8000], None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        let mut presenter = test_presenter(&config).unwrap();
        presenter.key_event(KeyCode::KeyF, ElementState::Pressed);
        presenter.advance(Duration::ZERO).unwrap();
        assert_eq!(presenter.assembled.machine.clock().0, 0);
        assert_eq!(presenter.assembled.machine.virtual_time(), 0);
        assert_eq!(presenter.keyboard.pending_events(), 1);
        assert!(presenter.input_log.is_empty());
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
                let Frame::Text(grid) = frame;
                *self.0.borrow_mut() = grid.to_dump();
            }
        }
        let required = std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES");
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
        let loaded = machine_file::load_for_assembly(
            default_machine(),
            assembly::AssemblyOptions::default(),
        )
        .unwrap();
        assert_eq!(loaded.config, assembly::bios(rom, Some(floppy)));
        let config = loaded.config;
        let args = parse_args([]).unwrap();
        let mut presenter = setup(&args, &mut io::sink()).unwrap();
        let assembled = &mut presenter.assembled;
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
        let start = presenter.assembled.machine.virtual_time();
        for _ in 0..400 {
            advance_frame(&mut presenter);
            assert!(presenter.running);
            if delivered.borrow().contains("2 file(s)") && presenter.keyboard.pending_events() == 0
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
        assert_eq!(presenter.keyboard.pending_events(), 0);
        assert!(presenter.assembled.machine.virtual_time() > start);
        assert!(presenter.sink.frame().is_some());
        assert!(presenter.window.is_none());
        let final_time = presenter.assembled.machine.virtual_time();
        let expected = guest_state(&mut presenter.assembled);
        let mut replay = observed_machine(&config);
        replay.machine.set_idle_limit(Some(u64::MAX));
        replay_input(&mut replay, &presenter.input_log, final_time, 31_337);
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
            let capture = presenter.assembled.capture_text(DeviceId::Mda).unwrap();
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
            assert!(presenter.sink.frame().is_some());
        }
        assert!(presenter.window.is_none());
    }
}
