// SPDX-License-Identifier: AGPL-3.0-or-later
//! Minimal FreeDOS window frontend (design/spec.md §9.4).
//!
//! Each redraw advances at most 100,000 virtual instruction equivalents through
//! the machine's scheduler, captures MDA through observation subscribers, and
//! paints the retained pixels. No host clock, keyboard injection or device tick
//! lives here. Reaching the FreeCOM date prompt freezes advancement for inspection.

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
    num::{NonZeroU16, NonZeroU32},
    path::{Path, PathBuf},
    rc::Rc,
};
use window::{WindowSink, FONT_NOTICE};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const CHUNK: u64 = 100_000;
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
    error: Option<Box<dyn Error>>,
}

impl Presenter {
    fn new(mut assembled: AssembledMachine) -> Result<Self, Box<dyn Error>> {
        // HLT must wait for an IRQ; each chunk's cap already bounds idle time.
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
            error: None,
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

    fn advance(&mut self) -> Result<(), Box<dyn Error>> {
        if !self.running {
            return Ok(());
        }
        let cap = self.assembled.machine.virtual_time().saturating_add(CHUNK);
        let outcome = self.assembled.machine.run_bounded(Some(cap));
        // Even a CPU error must leave the screen at its last completed boundary.
        let capture = self.assembled.capture_text(DeviceId::Mda)?;
        let video::Frame::Text(grid) = capture.frame;
        match outcome {
            Ok(RunOutcome::RetiredCapReached) => {
                // Keep the wording in sync with cli::DOS_DATE_PROMPT; wait for the full prompt.
                if grid.to_dump().contains("Enter new date (mm-dd-[cc]yy):") {
                    self.running = false;
                    eprintln!("freedos is waiting at the date prompt");
                }
            }
            Ok(RunOutcome::Halted) => {
                self.running = false;
                eprintln!("machine halted");
            }
            Err(error) => {
                self.running = false;
                eprintln!("machine stopped: {error}");
            }
        }
        if let Some(error) = self.sink.error() {
            return Err(error.into());
        }
        Ok(())
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
            }
        }
    }

    fn suspended(&mut self, _events: &ActiveEventLoop) {
        self.surface = None;
        self.window = None;
    }

    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window.as_ref().is_none_or(|window| window.id() != id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => events.exit(),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.advance().and_then(|()| self.paint()) {
                    self.fail(events, error);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _events: &ActiveEventLoop) {
        if self.running {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
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
    let policy = ObservationPolicy {
        routes: vec![ObservationRequest {
            device: DeviceId::Mda,
            output: OutputKind::TextScanout,
            byte_limit: 0,
        }],
        ..ObservationPolicy::default()
    };
    let assembled = assemble_with_observation(&config, &policy)?;
    let mut presenter = Presenter::new(assembled)?;
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

    /// Intel 80286 Programmer's Reference Manual, HLT; Intel 8254 datasheet,
    /// Mode 2: IRQ0 wakes the halted guest, whose handler acknowledges the PIC.
    #[test]
    fn timer_waiting_guest_advances_across_presenter_chunks_without_a_window() {
        // IVT[8] -> F800:0100; PIC IRQ0 enabled; PIT mode 2, divisor 60000.
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
            0xB0, 0x60, 0xE6, 0x40,
            0xB0, 0xEA, 0xE6, 0x40,
            0xFB, 0xF4, 0xEB, 0xFD,
        ];
        let mut rom = vec![0; 0x8000];
        rom[..program.len()].copy_from_slice(&program);
        rom[0x100..0x106].copy_from_slice(&[0x50, 0xB0, 0x20, 0xE6, 0x20, 0x58]);
        rom[0x106] = 0xCF;
        let mut config = assembly::bios(rom, None);
        config.cpu.reset = assembly::ResetSpec::Entry { cs: 0xF800, ip: 0 };
        config.cpu.segments = assembly::SegmentSeed::flat(0);
        let policy = ObservationPolicy {
            routes: vec![ObservationRequest {
                device: DeviceId::Mda,
                output: OutputKind::TextScanout,
                byte_limit: 0,
            }],
            ..ObservationPolicy::default()
        };
        let mut presenter =
            Presenter::new(assemble_with_observation(&config, &policy).unwrap()).unwrap();
        let mut previous_retired = 0;
        for chunk in 1..=3 {
            presenter.advance().unwrap();
            assert!(presenter.running);
            let capture = presenter.assembled.capture_text(DeviceId::Mda).unwrap();
            assert_eq!(capture.virtual_time, chunk * CHUNK);
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
