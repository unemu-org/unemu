// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure desktop commands, library discovery and texture conversion.
//!
//! Presentation consumes the existing machine-file validator and decoded frames;
//! see `backlog/docs/design/machine-images.md` §7 and `design/spec.md` §9.4.

use std::{
    io,
    path::{Path, PathBuf},
};
use winit::keyboard::KeyCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Stopped,
    Running,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Start,
    Pause,
    Stop,
    Reset,
    PowerCycle,
    Next(u8),
    Eject(u8),
    Mute,
    Release,
    Fullscreen,
    Library,
    Exit,
    Shortcuts,
    About,
    Placeholder(&'static str),
    SaveState,
    FirmwareFolders,
}

#[derive(Clone, Copy)]
pub(crate) struct Enablement {
    pub state: State,
    pub ready: bool,
    pub floppy: [bool; 2],
    pub audio: bool,
}

impl Enablement {
    pub fn allows(self, command: Command) -> bool {
        match command {
            Command::Start => self.state == State::Stopped && self.ready,
            Command::Pause
            | Command::Stop
            | Command::Reset
            | Command::PowerCycle
            | Command::Release => self.state != State::Stopped,
            Command::Next(unit) | Command::Eject(unit) => {
                self.state != State::Stopped
                    && self.floppy.get(unit as usize).copied().unwrap_or(false)
            }
            Command::Mute => self.state != State::Stopped && self.audio,
            Command::SaveState => false,
            _ => true,
        }
    }
}

pub(crate) fn hotkey(
    key: KeyCode,
    ctrl: bool,
    alt: bool,
    shift: bool,
    state: State,
    guest_focus: bool,
) -> Option<Command> {
    use Command::*;
    use KeyCode::*;
    if ctrl && alt {
        return Some(match key {
            KeyF => Next(u8::from(shift)),
            KeyE => Command::Eject(u8::from(shift)),
            KeyI => Placeholder("Insert floppy"),
            KeyM => Mute,
            KeyG => Release,
            KeyP => Command::Pause,
            KeyQ => Stop,
            End => Reset,
            Enter => Fullscreen,
            _ => return None,
        });
    }
    if state != State::Stopped && guest_focus {
        return None;
    }
    if ctrl && !alt {
        return match key {
            KeyN => Some(Placeholder("New machine")),
            KeyO => Some(Placeholder("Open machine file")),
            Comma => Some(Placeholder("Settings")),
            _ => None,
        };
    }
    match key {
        F1 => Some(Shortcuts),
        Enter if state == State::Stopped && !alt && !ctrl => Some(Start),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub details: Vec<(String, String)>,
    pub warning: Option<String>,
    pub ems: String,
    pub rate: String,
    pub floppy: [bool; 2],
    pub checked: bool,
    pub discovery_warnings: Vec<String>,
}

impl LibraryEntry {
    pub fn pending(path: PathBuf) -> Self {
        Self {
            name: machine_file::resolve(&path)
                .ok()
                .and_then(|(file, _)| file.name)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                }),
            path,
            details: Vec::new(),
            warning: Some("Checking firmware…".into()),
            ems: "EMS absent".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: false,
            discovery_warnings: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn inspect(path: PathBuf, options: assembly::AssemblyOptions) -> Self {
        Self::inspect_with_rom_dirs(path, options, &[])
    }

    #[cfg(test)]
    pub fn inspect_with_rom_dirs(
        path: PathBuf,
        options: assembly::AssemblyOptions,
        roots: &[PathBuf],
    ) -> Self {
        let roots = roots
            .iter()
            .map(|path| {
                assembly::firmware_discovery::FirmwareRoot::new(
                    path,
                    assembly::firmware_discovery::RootOrigin::CommandLine,
                )
            })
            .collect::<Vec<_>>();
        Self::prepare(path, options, &roots).0
    }

    fn prepare(
        path: PathBuf,
        options: assembly::AssemblyOptions,
        roots: &[assembly::firmware_discovery::FirmwareRoot],
    ) -> PreparedEntry {
        let name = machine_file::resolve(&path)
            .ok()
            .and_then(|(file, _)| file.name)
            .unwrap_or_else(|| {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        let mut entry = Self {
            path,
            name,
            details: Vec::new(),
            warning: None,
            ems: "EMS absent".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: true,
            discovery_warnings: Vec::new(),
        };
        let mut prepared = None;
        match machine_file::prepare_for_assembly_with_firmware_roots(
            &entry.path,
            roots,
            &mut entry.discovery_warnings,
        )
        .and_then(|document| document.load(options).map(|loaded| (document, loaded)))
        {
            Ok((document, loaded)) => {
                let config = &loaded.config;
                entry.rate = format!(
                    "80286 · {}/{}",
                    config.clock.retired_per_pit_clock,
                    config.clock.retired_per_pit_clock_denominator
                );
                let memory: u64 = config.memory.ram.iter().map(|ram| ram.size).sum();
                let display = loaded
                    .cards
                    .iter()
                    .filter(|(_, id)| {
                        matches!(id, assembly::DeviceId::Mda | assembly::DeviceId::Vga)
                    })
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let sound = config
                    .devices
                    .iter()
                    .any(|device| matches!(device.kind, assembly::DeviceKind::Sb16(_)));
                for device in &config.devices {
                    if let assembly::DeviceKind::LotechEms(ems) = device.kind {
                        entry.ems = format!("EMS {} KiB", ems.size_kib);
                    }
                }
                for drive in &config.drives {
                    if drive.controller == assembly::DeviceId::Fdc && drive.unit < 2 {
                        entry.floppy[drive.unit as usize] = true;
                    }
                }
                entry.details = vec![
                    ("Board".into(), config.board.id().into()),
                    ("Memory".into(), format!("{} KiB RAM", memory / 1024)),
                    ("Display".into(), display),
                    (
                        "Sound".into(),
                        if sound { "SB16 / OPL3" } else { "PC speaker" }.into(),
                    ),
                    (
                        "Drives".into(),
                        config
                            .drives
                            .iter()
                            .map(|drive| {
                                format!("{:?} {}: {:?}", drive.controller, drive.unit, drive.kind)
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    ),
                ];
                if let Err(error) = loaded.check() {
                    entry.warning = Some(error.to_string());
                } else {
                    prepared = Some(document);
                }
            }
            Err(error) => entry.warning = Some(error.to_string()),
        }
        (entry, prepared)
    }
}

pub(crate) type PreparedEntry = (LibraryEntry, Option<machine_file::PreparedMachineFile>);

/// Only host loading/checking runs on this worker. Guest assembly, audio and
/// execution remain on the frontend's existing execution path.
pub(crate) fn spawn_library_check(
    path: PathBuf,
    options: assembly::AssemblyOptions,
    roots: Vec<assembly::firmware_discovery::FirmwareRoot>,
    wake: impl FnOnce() + Send + 'static,
) -> std::sync::mpsc::Receiver<PreparedEntry> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let prepared = LibraryEntry::prepare(path, options, &roots);
        let _ = tx.send(prepared);
        wake();
    });
    rx
}

pub(crate) fn machine_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for dir in [root.to_path_buf(), root.join("local")] {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let path = entry?.path();
            if path.is_file()
                && matches!(
                    path.extension().and_then(|value| value.to_str()),
                    Some("toml" | "yaml" | "yml")
                )
            {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

pub(crate) fn select_machine(paths: &mut Vec<PathBuf>, requested: &Path) -> usize {
    if let Some(index) = paths.iter().position(|path| {
        path == requested
            || path
                .canonicalize()
                .ok()
                .zip(requested.canonicalize().ok())
                .is_some_and(|(left, right)| left == right)
    }) {
        return index;
    }
    paths.push(requested.to_path_buf());
    paths.len() - 1
}

pub(crate) fn integer_size(
    available: egui::Vec2,
    texture: egui::Vec2,
    sink_scale: u16,
) -> egui::Vec2 {
    let original = texture / f32::from(sink_scale);
    let fit = (available.x / original.x).min(available.y / original.y);
    original * if fit >= 1.0 { fit.floor() } else { fit }
}

pub(crate) fn texture_image(frame: &window::PixelFrame) -> egui::ColorImage {
    let mut rgb = Vec::with_capacity(frame.pixels.len() * 3);
    for pixel in &frame.pixels {
        rgb.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, *pixel as u8]);
    }
    egui::ColorImage::from_rgb([frame.width as usize, frame.height as usize], &rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn firmware_readiness_is_lazy_and_runs_on_a_host_worker() {
        let temp = assembly::test_support::TempDir::new("worker-firmware-check");
        let path = temp.path().join("missing.toml");
        let entry = LibraryEntry::pending(path.clone());
        assert!(!entry.checked);
        assert_eq!(entry.warning.as_deref(), Some("Checking firmware…"));
        let caller = std::thread::current().id();
        let (tx, rx) = std::sync::mpsc::channel();
        let result = spawn_library_check(
            path,
            assembly::AssemblyOptions::default(),
            vec![],
            move || {
                tx.send(std::thread::current().id()).unwrap();
            },
        )
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
        assert_ne!(
            rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap(),
            caller
        );
        assert!(result.0.checked);
        assert!(result.0.warning.is_some());
        assert!(result.1.is_none());
    }

    #[test]
    fn discovery_includes_local_and_yaml_but_not_nested_or_non_machine_files() {
        let temp = assembly::test_support::TempDir::new("shell-discovery");
        std::fs::create_dir_all(temp.path().join("local/nested")).unwrap();
        for file in [
            "z.toml",
            "a.yaml",
            "b.yml",
            "local/c.toml",
            "README.md",
            "local/nested/hidden.toml",
        ] {
            std::fs::write(temp.path().join(file), "").unwrap();
        }
        let names: Vec<_> = machine_paths(temp.path())
            .unwrap()
            .iter()
            .map(|path| {
                path.strip_prefix(temp.path())
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["a.yaml", "b.yml", "local/c.toml", "z.toml"]);
        assert!(machine_paths(&temp.path().join("absent"))
            .unwrap()
            .is_empty());
    }
    #[test]
    fn selection_keeps_a_missing_requested_machine_instead_of_starting_another() {
        let mut paths = vec![PathBuf::from("first.toml")];
        assert_eq!(select_machine(&mut paths, Path::new("first.toml")), 0);
        assert_eq!(paths.len(), 1);
        let selected = select_machine(&mut paths, Path::new("missing.toml"));
        assert_eq!(paths[selected], Path::new("missing.toml"));
    }

    #[test]
    fn enablement_requires_ready_firmware_and_live_capabilities() {
        for state in [State::Stopped, State::Running, State::Paused] {
            for ready in [false, true] {
                let model = Enablement {
                    state,
                    ready,
                    floppy: [true, false],
                    audio: true,
                };
                assert_eq!(
                    model.allows(Command::Start),
                    state == State::Stopped && ready
                );
                for command in [
                    Command::Pause,
                    Command::Stop,
                    Command::Reset,
                    Command::PowerCycle,
                    Command::Next(0),
                    Command::Eject(0),
                    Command::Mute,
                ] {
                    assert_eq!(model.allows(command), state != State::Stopped);
                }
                assert!(!model.allows(Command::Eject(1)));
                assert!(!model.allows(Command::SaveState));
            }
        }
        assert!(!Enablement {
            state: State::Running,
            ready: true,
            floppy: [false; 2],
            audio: false
        }
        .allows(Command::Mute));
    }
    #[test]
    fn hotkeys_preserve_media_shift_and_leave_plain_guest_keys_alone() {
        use KeyCode::*;
        for (key, command) in [
            (KeyF, Command::Next(0)),
            (KeyE, Command::Eject(0)),
            (KeyM, Command::Mute),
            (KeyG, Command::Release),
            (KeyP, Command::Pause),
            (KeyQ, Command::Stop),
            (End, Command::Reset),
            (Enter, Command::Fullscreen),
        ] {
            assert_eq!(
                hotkey(key, true, true, false, State::Running, true),
                Some(command)
            );
        }
        assert_eq!(
            hotkey(KeyF, true, true, true, State::Running, true),
            Some(Command::Next(1))
        );
        assert_eq!(
            hotkey(KeyE, true, true, true, State::Running, true),
            Some(Command::Eject(1))
        );
        assert_eq!(
            hotkey(KeyF, false, false, false, State::Running, true),
            None
        );
        assert_eq!(
            hotkey(Enter, false, false, false, State::Running, true),
            None
        );
        assert_eq!(
            hotkey(Enter, false, false, false, State::Stopped, true),
            Some(Command::Start)
        );
    }
    #[test]
    fn texture_upload_preserves_dimensions_channels_and_row_order() {
        let image = texture_image(&window::PixelFrame {
            width: 2,
            height: 2,
            pixels: vec![0xff0000, 0x00ff00, 0x0000ff, 0x123456],
        });
        assert_eq!(image.size, [2, 2]);
        assert_eq!(
            image.pixels,
            [
                egui::Color32::RED,
                egui::Color32::GREEN,
                egui::Color32::BLUE,
                egui::Color32::from_rgb(0x12, 0x34, 0x56)
            ]
        );
    }
    #[test]
    fn invalid_machine_is_visible_with_start_blocked() {
        let temp = assembly::test_support::TempDir::new("shell-invalid");
        let path = temp.path().join("missing-firmware.toml");
        std::fs::write(&path, "schema = 1\nname = 'Missing firmware'\nextends = 'preset:ibm-5170'\n[firmware.bios]\nimage = 'absent.bin'\n").unwrap();
        let entry = LibraryEntry::inspect(path, assembly::AssemblyOptions::default());
        assert_eq!(entry.name, "Missing firmware");
        assert!(entry.warning.as_deref().unwrap().contains("absent.bin"));
    }
    #[test]
    fn ordinary_shortcuts_belong_to_a_focused_guest() {
        for state in [State::Stopped, State::Running, State::Paused] {
            for focus in [true, false] {
                for (key, ctrl, command) in [
                    (KeyCode::F1, false, Command::Shortcuts),
                    (KeyCode::KeyN, true, Command::Placeholder("New machine")),
                    (
                        KeyCode::KeyO,
                        true,
                        Command::Placeholder("Open machine file"),
                    ),
                    (KeyCode::Comma, true, Command::Placeholder("Settings")),
                ] {
                    assert_eq!(
                        hotkey(key, ctrl, false, false, state, focus),
                        if state != State::Stopped && focus {
                            None
                        } else {
                            Some(command)
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn integer_scale_uses_native_pixels_and_fits_a_small_client() {
        let texture = egui::vec2(1440.0, 800.0);
        assert_eq!(
            integer_size(egui::vec2(1000.0, 500.0), texture, 2),
            egui::vec2(720.0, 400.0)
        );
        assert_eq!(integer_size(egui::vec2(1500.0, 900.0), texture, 2), texture);
        assert_eq!(
            integer_size(egui::vec2(360.0, 200.0), texture, 2),
            egui::vec2(360.0, 200.0)
        );
    }
}
