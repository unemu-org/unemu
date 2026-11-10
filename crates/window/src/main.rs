// SPDX-License-Identifier: AGPL-3.0-or-later
//! Minimal FreeDOS window frontend (design/spec.md §9.4).
//!
//! The event loop alone reads a monotonic host clock. A pure pacing policy
//! converts elapsed time to bounded PIT targets and virtual-time run caps.
//! Input enters only at run boundaries; observation captures the completed
//! boundary. See `backlog/docs/design/observation.md` § Window pacing.

use assembly::{
    assemble_with_observation, AssembledMachine, DeviceId, ObservationPolicy, ObservationRequest,
    OutputKind,
};
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
const BIOS_FIXTURE: &str = "vendor/kiselev-bios/bios-xi8088.bin";
const FLOPPY_FIXTURE: &str = "vendor/freedos/freedos.img";
const BIOS_FETCH: &str = "bash vendor/kiselev-bios/fetch.sh";
const FLOPPY_FETCH: &str = "bash vendor/freedos/fetch.sh";

type HostSurface = Surface<Rc<Window>, Rc<Window>>;

struct Presenter {
    assembled: AssembledMachine,
    sink: WindowSink,
    surface: Option<HostSurface>,
    window: Option<Rc<Window>>,
    running: bool,
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
    fn new(config: &assembly::MachineConfig) -> Result<Self, Box<dyn Error>> {
        let mut assembled = assemble_with_observation(
            config,
            &ObservationPolicy {
                routes: vec![ObservationRequest {
                    device: DeviceId::Mda,
                    output: OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..ObservationPolicy::default()
            },
        )?;
        let clock = &config.clock;
        // HLT must wait for an IRQ; the pacing cap already bounds idle time.
        assembled.machine.set_idle_limit(Some(u64::MAX));
        let sink = WindowSink::new(NonZeroU16::new(2).ok_or("invalid scale")?)?;
        assembled
            .observation
            .subscribe(DeviceId::Mda, Box::new(sink.clone()))?;
        Ok(Self {
            assembled,
            sink,
            surface: None,
            window: None,
            running: true,
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
        if !self.running || self.occluded_since.is_some() {
            return Ok(());
        }
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
            let delivered = self.keyboard.inject_at_boundary(&mut self.assembled);
            #[cfg(test)]
            if !delivered.is_empty() {
                self.input_log.push((time, delivered));
            }
            #[cfg(not(test))]
            drop(delivered);
            let outcome = self.assembled.machine.run_bounded(Some(cap));
            match outcome {
                Ok(RunOutcome::RetiredCapReached) => self.running = cap < u64::MAX,
                Ok(RunOutcome::Halted) => {
                    self.running = false;
                    eprintln!("machine halted");
                }
                Err(error) => {
                    self.running = false;
                    eprintln!("machine stopped: {error}");
                }
            }
        }
        // Even a CPU error must leave the screen at its last completed boundary.
        self.assembled.capture_text(DeviceId::Mda)?;
        if let Some(error) = self.sink.error() {
            return Err(error.into());
        }
        Ok(())
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
            "usage: unemupc98-window [bios-path [floppy-path]]\n\n{FONT_NOTICE}"
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

fn default_fixture(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

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

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if let Some(output) = first.as_deref().and_then(informational_output) {
        println!("{output}");
        return Ok(());
    }
    let bios = first
        .map(PathBuf::from)
        .unwrap_or_else(|| default_fixture(BIOS_FIXTURE));
    let floppy = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_fixture(FLOPPY_FIXTURE));
    if args.next().is_some() {
        return Err("usage: unemupc98-window [bios-path [floppy-path]]".into());
    }
    let config = assembly::bios(
        read_fixture(&bios, BIOS_FETCH)?,
        Some(read_fixture(&floppy, FLOPPY_FETCH)?),
    );
    let mut presenter = Presenter::new(&config)?;
    let events = EventLoop::new()?;
    events.set_control_flow(ControlFlow::Wait);
    events.run_app(&mut presenter)?;
    if let Some(error) = presenter.error {
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut presenter = Presenter::new(&assembly::bios(vec![0; 0x8000], None)).unwrap();
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
        let mut presenter = Presenter::new(&config).unwrap();
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
        let mut presenter = Presenter::new(&config).unwrap();
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
            let mut paced = Presenter::new(&config).unwrap();
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
            let mut presenter = Presenter::new(&config).unwrap();
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
        let mut presenter = Presenter::new(&timer_guest_config()).unwrap();
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
        let mut presenter = Presenter::new(&config).unwrap();
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
        let config = assembly::bios(rom, Some(floppy));
        let mut presenter = Presenter::new(&config).unwrap();
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
        let mut presenter = Presenter::new(&config).unwrap();
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
