// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure desktop commands, library discovery and texture conversion.
//!
//! Presentation consumes the existing machine-file validator and decoded frames;
//! see `backlog/docs/design/machine-images.md` §7 and `design/spec.md` §9.4.

use std::{
    io,
    path::{Path, PathBuf},
};
use winit::keyboard::{Key, KeyCode, NamedKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Stopped,
    Running,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Start,
    GamingMode,
    Layers,
    ImportImage,
    ImportFirmware,
    /// File › Edit metadata… on the selected firmware package.
    EditFirmware,
    /// File › Import installation media….
    ImportMedia,
    DeleteTag,
    StoreFolder,
    Pause,
    Stop,
    Reset,
    PowerCycle,
    /// Insert… of a floppy drive: the media picker.
    Insert(u8),
    /// Create from files… of a floppy drive: the media picker in its Create
    /// view.
    CreateFromFiles(u8),
    /// Browse for an image file… of the media picker: a disk image file on
    /// this computer, or a disk of the drive's catalogue, with its write mode.
    InsertFile(u8),
    /// Insert of the set selected in the library window: the media picker
    /// with that set chosen.
    InsertSet(u8),
    /// Change the write mode of the floppy in a drive, in its Insert dialog.
    WriteMode(u8),
    CreateBlank,
    Next(u8),
    Eject(u8),
    Mute,
    Speed(crate::pacing::Speed),
    FastForward,
    Release,
    Fullscreen,
    Library,
    Exit,
    Minimize,
    Shortcuts,
    About,
    Placeholder(&'static str),
    SaveState,
    FirmwareFolders,
    Preferences,
    Settings,
    /// Machine › Save as new image…: the Disk layers save form.
    SaveImage,
    /// Image › Push… of a local image to a registry.
    Push,
    /// Image › Pull… of a registry image into the local store.
    Pull,
    /// File › Get templates…: pull official templates from the default registry.
    GetTemplates,
    /// Rename… of the selected machine (F2).
    Rename,
    /// Duplicate… of the selected machine (Ctrl+D).
    Duplicate,
    /// Delete machine… of the selected machine.
    DeleteMachine,
    /// The Delete key: Delete machine… on a machine, Delete tag… on an image.
    DeleteSelection,
    /// Reset to image… of the selected machine.
    ResetMachine,
    /// Machine › Show source image.
    ShowSource,
    /// New machine from image… (Ctrl+Shift+N).
    NewFromImage,
    /// Image › Start…: the selected image, or the selected machine's source.
    ImageStart,
    /// Image › Gaming mode: a disposable copy of the image itself.
    ImageGaming,
    /// Image › Settings…: a new revision of the image.
    ImageSettings,
    /// Image › Layers…: the image's own chains.
    ImageLayers,
}

impl Command {
    /// The library's own keys, which an edited text field keeps for itself.
    pub fn library_key(self) -> bool {
        matches!(
            self,
            Command::Rename
                | Command::Duplicate
                | Command::DeleteSelection
                | Command::NewFromImage
                | Command::Start
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Enablement {
    pub state: State,
    pub ready: bool,
    /// Drives that take a floppy now: those of the session, or while stopped,
    /// those of the selected machine, whose floppies Start puts in.
    pub floppy: [bool; 2],
    pub audio: bool,
}

impl Enablement {
    pub fn allows(self, command: Command) -> bool {
        match command {
            Command::Start | Command::GamingMode | Command::ImageStart | Command::ImageGaming => {
                self.state == State::Stopped && self.ready
            }
            Command::Duplicate
            | Command::DeleteMachine
            | Command::DeleteSelection
            | Command::ResetMachine
            | Command::ShowSource
            | Command::NewFromImage => self.state == State::Stopped,
            Command::Pause
            | Command::Stop
            | Command::Reset
            | Command::PowerCycle
            | Command::Release
            | Command::Speed(_)
            | Command::FastForward => self.state != State::Stopped,
            Command::Insert(unit)
            | Command::CreateFromFiles(unit)
            | Command::InsertFile(unit)
            | Command::Eject(unit) => self.floppy.get(unit as usize).copied().unwrap_or(false),
            Command::InsertSet(unit) | Command::WriteMode(unit) | Command::Next(unit) => {
                self.state != State::Stopped
                    && self.floppy.get(unit as usize).copied().unwrap_or(false)
            }
            Command::Mute => self.state != State::Stopped && self.audio,
            Command::SaveState => false,
            _ => true,
        }
    }
}

pub(crate) fn logical_shortcut_key(key: &Key) -> Option<KeyCode> {
    use KeyCode::*;
    match key {
        Key::Character(text) => match text.to_lowercase().as_str() {
            "f" => Some(KeyF),
            "d" => Some(KeyD),
            "e" => Some(KeyE),
            "i" => Some(KeyI),
            "m" => Some(KeyM),
            "k" => Some(KeyK),
            "w" => Some(KeyW),
            "t" => Some(KeyT),
            "g" => Some(KeyG),
            "p" => Some(KeyP),
            "q" => Some(KeyQ),
            "n" => Some(KeyN),
            "o" => Some(KeyO),
            "," => Some(Comma),
            "/" | "?" => Some(Slash),
            _ => None,
        },
        Key::Named(NamedKey::End) => Some(End),
        Key::Named(NamedKey::Enter) => Some(Enter),
        Key::Named(NamedKey::F2) => Some(F2),
        Key::Named(NamedKey::Delete) => Some(Delete),
        Key::Named(NamedKey::Backspace) => Some(Backspace),
        _ => None,
    }
}

pub(crate) fn hotkey(
    key: KeyCode,
    modifiers: [bool; 8],
    state: State,
    guest_focus: bool,
    macos: bool,
) -> Option<Command> {
    use Command::*;
    use KeyCode::*;
    let ctrl = modifiers[..2].contains(&true);
    let alt = modifiers[2..4].contains(&true);
    let shift = modifiers[4..6].contains(&true);
    let command = modifiers[6..].contains(&true);
    if macos && command {
        if ctrl || alt {
            return None;
        }
        // The library's keys act outside guest focus, as the machines mock-up
        // says; whether the command applies then is its enablement's call.
        let library = state == State::Stopped || !guest_focus;
        return match (key, shift) {
            (KeyN, true) if library => Some(NewFromImage),
            (KeyD, false) if library => Some(Duplicate),
            (Backspace, false) if library => Some(DeleteSelection),
            (KeyF, _) => Some(Next(u8::from(shift))),
            (KeyE, _) => Some(Command::Eject(u8::from(shift))),
            (KeyI, _) => Some(Command::Insert(u8::from(shift))),
            (KeyM, true) => Some(Mute),
            (KeyM, false) => Some(Minimize),
            (KeyK, false) => Some(Stop),
            (KeyQ, false) | (KeyW, false) => Some(Exit),
            (KeyT, false) => Some(FastForward),
            (KeyG, false) => Some(Release),
            (KeyP, false) => Some(Command::Pause),
            (End, false) => Some(Reset),
            (Enter, false) => Some(Fullscreen),
            (KeyN, false) => Some(Placeholder("New machine")),
            (KeyO, false) => Some(Placeholder("Open machine file")),
            (Comma, false) => Some(Settings),
            (Slash, true) => Some(Shortcuts),
            _ => None,
        };
    }
    if !macos && ctrl && alt {
        return Some(match key {
            KeyF => Next(u8::from(shift)),
            KeyE => Command::Eject(u8::from(shift)),
            KeyI => Command::Insert(u8::from(shift)),
            KeyM => Mute,
            KeyT => FastForward,
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
    if !macos && ctrl && !alt {
        return match key {
            KeyN if shift => Some(NewFromImage),
            KeyD if !shift => Some(Duplicate),
            KeyN => Some(Placeholder("New machine")),
            KeyO => Some(Placeholder("Open machine file")),
            Comma => Some(Settings),
            _ => None,
        };
    }
    let plain = !alt && !ctrl && !shift && !command;
    match key {
        F1 if !macos => Some(Shortcuts),
        Enter if state == State::Stopped && !alt && !ctrl => Some(Start),
        F2 if plain => Some(Rename),
        Delete if plain && !macos => Some(DeleteSelection),
        _ => None,
    }
}

/// Use the same labels in menus and help so platform-specific chords stay visible.
pub(crate) fn shortcut_text(shortcut: &str, macos: bool) -> &str {
    if !macos {
        return shortcut;
    }
    match shortcut {
        "Ctrl+Alt+P" => "⌘P",
        "Ctrl+Alt+T" => "⌘T",
        "Ctrl+Alt+Q" => "⌘K",
        "Ctrl+Alt+End" => "⌘End",
        "Ctrl+Alt+Enter" => "⌘Enter",
        "Ctrl+Alt+M" => "⌘⇧M",
        "Ctrl+Alt+G" => "⌘G",
        "Ctrl+Alt+F" => "⌘F",
        "Ctrl+Alt+Shift+F" => "⌘⇧F",
        "Ctrl+Alt+E" => "⌘E",
        "Ctrl+Alt+Shift+E" => "⌘⇧E",
        "Ctrl+Alt+I" => "⌘I",
        "Ctrl+Alt+Shift+I" => "⌘⇧I",
        "F1" => "⌘⇧/",
        "Ctrl+N" => "⌘N",
        "Ctrl+Shift+N" => "⌘⇧N",
        "Ctrl+D" => "⌘D",
        "Delete" => "⌘⌫",
        "Ctrl+O" => "⌘O",
        "Ctrl+," => "⌘,",
        _ => shortcut,
    }
}

#[derive(Clone, Debug)]
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
    source: Option<String>,
}

impl LibraryEntry {
    pub fn pending(path: PathBuf) -> Self {
        let source = source_revision(&path);
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
            ems: "EMS unknown".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: false,
            discovery_warnings: Vec::new(),
            source,
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
        Self::prepare(path, options, &roots, None, None).0
    }

    fn prepare(
        path: PathBuf,
        options: assembly::AssemblyOptions,
        roots: &[assembly::firmware_discovery::FirmwareRoot],
        control: Option<&assembly::firmware_discovery::DiscoveryControl>,
        store_root: Option<&Path>,
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
        let source = source_revision(&path);
        let mut entry = Self {
            path,
            name,
            details: Vec::new(),
            warning: None,
            ems: "EMS unknown".into(),
            rate: String::new(),
            floppy: [false; 2],
            checked: true,
            discovery_warnings: Vec::new(),
            source,
        };
        let mut prepared = None;
        match machine_file::prepare_for_assembly_with_store_root(
            &entry.path,
            roots,
            &mut entry.discovery_warnings,
            store_root,
        )
        .and_then(|document| {
            if let Some(control) = control {
                control.loading();
            }
            document.load(options).map(|loaded| (document, loaded))
        }) {
            Ok((document, loaded)) => {
                let config = &loaded.config;
                entry.ems = "EMS absent".into();
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
        if let Some(warning) = &entry.warning {
            entry.warning = Some(relative_diagnostic(warning, &entry.path));
        }
        (entry, prepared)
    }
}

impl LibraryEntry {
    pub fn has_problem(&self) -> bool {
        self.checked && self.warning.is_some()
    }

    pub fn hint(&self) -> &str {
        if !self.checked {
            return "Checking machine configuration…";
        }
        match self.warning.as_deref() {
            Some(warning) if warning.contains("cancelled") =>
                "Use Tools › Locate firmware… › Check again for a normal cached firmware check.",
            Some(warning) if warning.contains("cannot read image") && warning.contains("drives[") =>
                "Media image is missing or unreadable. Use Tools › Create blank disk… for a new disk, or correct the image path in the machine file.",
            Some(warning) if (warning.contains("SHA-256") || warning.contains("sha256")) && warning.contains("drives[") =>
                "Media image has the wrong SHA-256. Restore the expected image or correct its pin in the machine file.",
            Some(warning) if warning.contains("firmware") || warning.contains("ROM") || warning.contains("bios") =>
                "Required firmware is missing, unreadable or incompatible. Use Tools › Locate firmware… and check the firmware declarations.",
            Some(_) if self.checked => "Correct the reported configuration field in the machine file.",
            _ => "Checking machine configuration…",
        }
    }
}

pub(crate) fn relative_diagnostic(text: &str, machine: &Path) -> String {
    let root = machine
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "machines"))
        .or_else(|| machine.parent())
        .unwrap_or(Path::new("."));
    let mut result = text.to_owned();
    for root in [
        Some(root.to_path_buf()),
        root.canonicalize().ok(),
        std::path::absolute(root).ok(),
    ]
    .into_iter()
    .flatten()
    {
        let prefix = format!("{}/", root.display());
        result = result.replace(&prefix, "");
    }
    result
}

/// Narrowest drive area, the width of the former A: and C: placeholders.
const STATUS_DRIVES_MIN: f32 = 80.0;
/// Widest drive area: three full-width chips.
const STATUS_DRIVES_MAX: f32 = 900.0;
/// Spare width the message keeps before the drive area grows, so the
/// pointer-release hint stays readable at the default window size.
const STATUS_MESSAGE_FIRST: f32 = 240.0;

/// Message, power, drive chips, CPU, EMS, audio and speed cells. The message
/// keeps the first spare width, the drive area takes the next up to three full
/// chips, and the rest returns to the message. Widths depend only on the window
/// width, never on the drives' contents, so chips appearing or changing never
/// move the other cells. CPU, EMS and speed are sized to their longest usual
/// text ("80286 · 32/39", "EMS 2048 KiB", "Speed 100% 1.0x"), which leaves the
/// drive area room at the default window for an A: and a C: chip with their
/// format and write mode. Every cell is clipped independently even on a
/// narrow client.
pub(crate) fn status_widths(available: f32) -> [f32; 7] {
    let fixed = [68.0, STATUS_DRIVES_MIN, 100.0, 92.0, 118.0, 124.0];
    let total: f32 = fixed.iter().sum();
    let scale = (available.max(0.0) / total).min(1.0);
    let mut widths = [0.0; 7];
    for (width, fixed) in widths[1..].iter_mut().zip(fixed) {
        *width = fixed * scale;
    }
    let spare = (available - total).max(0.0);
    let drives = (spare - STATUS_MESSAGE_FIRST).clamp(0.0, STATUS_DRIVES_MAX - STATUS_DRIVES_MIN);
    widths[2] += drives;
    widths[0] = spare - drives;
    widths
}

fn source_revision(path: &Path) -> Option<String> {
    machine_file::resolve(path)
        .ok()
        .map(|(file, origins)| format!("{file:?}{origins:?}"))
}

fn failed_firmware_paths(
    document: &machine_file::PreparedMachineFile,
    field: &str,
) -> Vec<PathBuf> {
    let (document, origins) = document.resolved();
    let mut paths = Vec::new();
    if let Some(firmware) = &document.firmware {
        let roms = firmware
            .bios
            .iter()
            .map(|rom| ("firmware.bios".into(), rom))
            .chain(
                firmware
                    .option_roms
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, rom)| (format!("firmware.option_roms[{index}]"), rom)),
            );
        for (rom_field, rom) in roms {
            if !field.starts_with(&rom_field) {
                continue;
            }
            if let Some(image) = &rom.image {
                paths.push(origins.image_path(&format!("{rom_field}.image"), image));
            }
            for (index, part) in rom.parts.iter().flatten().enumerate() {
                let part_field = format!("{rom_field}.parts[{index}]");
                if !field.contains(".parts[") || field.starts_with(&part_field) {
                    paths.push(origins.image_path(&format!("{part_field}.image"), &part.image));
                }
            }
        }
    }
    paths
}

pub(crate) type PreparedEntry = (LibraryEntry, Option<machine_file::PreparedMachineFile>);

/// Only host loading/checking runs on this worker. Guest assembly, audio and
/// execution remain on the frontend's existing execution path.
pub(crate) struct LibraryCheck {
    pub path: PathBuf,
    pub options: assembly::AssemblyOptions,
    pub roots: Vec<assembly::firmware_discovery::FirmwareRoot>,
    pub cache: Option<PathBuf>,
    pub store_root: Option<PathBuf>,
    pub rescan: bool,
    pub refresh_listing: bool,
    pub control: assembly::firmware_discovery::DiscoveryControl,
    pub session: assembly::firmware_discovery::DiscoverySession,
    pub prepared: Option<PreparedEntry>,
}

pub(crate) fn spawn_library_check(
    request: LibraryCheck,
    wake: impl FnOnce() + Send + 'static,
) -> std::sync::mpsc::Receiver<PreparedEntry> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let LibraryCheck {
            path,
            options,
            roots,
            cache,
            store_root,
            rescan,
            refresh_listing,
            control,
            session,
            prepared,
        } = request;
        let roots = roots
            .into_iter()
            .map(|root| {
                assembly::firmware_discovery::FirmwareRoot::new(
                    std::path::absolute(&root.path).unwrap_or(root.path),
                    root.origin,
                )
            })
            .collect::<Vec<_>>();
        let prepared = session.run(cache.as_deref(), rescan, &control, || {
            if control.is_cancelled() {
                return (LibraryEntry::pending(path), None);
            }
            if refresh_listing {
                assembly::firmware_discovery::refresh_firmware_roots(&roots);
            }
            if let Some((mut entry, Some(document))) = prepared {
                if entry.source.is_some() && entry.source == source_revision(&path) {
                    control.loading();
                    match document
                        .load(options)
                        .and_then(|loaded| loaded.check().map(|_| ()).map_err(Into::into))
                    {
                        Ok(()) => return (entry, Some(document)),
                        Err(error) => {
                            let failed_paths = match &error {
                                machine_file::LoadError::Lower(error)
                                    if error.field.starts_with("firmware.") =>
                                {
                                    Some(failed_firmware_paths(&document, &error.field))
                                }
                                _ => None,
                            };
                            let error = error.to_string();
                            if let Some(paths) = failed_paths {
                                assembly::firmware_discovery::refresh_changed_firmware_roots(
                                    &roots, &paths,
                                );
                            } else {
                                entry.warning = Some(relative_diagnostic(&error, &path));
                                return (entry, None);
                            }
                        }
                    }
                }
            }
            LibraryEntry::prepare(path, options, &roots, Some(&control), store_root.as_deref())
        });
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
            let visible = path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                !name.starts_with('.') && !name.contains(".pending.")
            });
            if visible
                && path.is_file()
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

/// The library row selected at start. A machine named with `--machine`
/// (`given`) is added even when missing, so its error shows; the default
/// machine only when its file exists, so a fresh install selects nothing.
pub(crate) fn select_machine(
    paths: &mut Vec<PathBuf>,
    requested: &Path,
    given: bool,
) -> Option<usize> {
    if let Some(index) = paths.iter().position(|path| {
        path == requested
            || path
                .canonicalize()
                .ok()
                .zip(requested.canonicalize().ok())
                .is_some_and(|(left, right)| left == right)
    }) {
        return Some(index);
    }
    if !given && !requested.is_file() {
        return None;
    }
    paths.push(requested.to_path_buf());
    Some(paths.len() - 1)
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
    fn hotkey(
        key: winit::keyboard::KeyCode,
        ctrl: bool,
        alt: bool,
        shift: bool,
        state: super::State,
        guest_focus: bool,
    ) -> Option<super::Command> {
        super::hotkey(
            key,
            [ctrl, false, alt, false, shift, false, false, false],
            state,
            guest_focus,
            false,
        )
    }

    use super::*;
    #[test]
    fn macos_logical_shortcut_letters_and_punctuation_match_menu_labels() {
        use Command::*;
        for (text, shift, expected) in [
            ("f", false, Next(0)),
            ("F", true, Next(1)),
            ("e", false, Eject(0)),
            ("E", true, Eject(1)),
            ("i", false, Insert(0)),
            ("I", true, Insert(1)),
            ("m", false, Minimize),
            ("M", true, Mute),
            ("k", false, Stop),
            ("q", false, Exit),
            ("w", false, Exit),
            ("t", false, FastForward),
            ("g", false, Release),
            ("p", false, Pause),
            ("n", false, Placeholder("New machine")),
            ("o", false, Placeholder("Open machine file")),
            (",", false, Settings),
            ("?", true, Shortcuts),
        ] {
            let key = logical_shortcut_key(&Key::Character(text.into())).unwrap();
            assert_eq!(
                super::hotkey(
                    key,
                    [false, false, false, false, shift, false, true, false],
                    State::Running,
                    true,
                    true
                ),
                Some(expected)
            );
        }
        for text in ["a", "z", "é"] {
            assert_eq!(logical_shortcut_key(&Key::Character(text.into())), None);
        }
        for (named, expected) in [(NamedKey::End, Reset), (NamedKey::Enter, Fullscreen)] {
            let key = logical_shortcut_key(&Key::Named(named)).unwrap();
            assert_eq!(
                super::hotkey(
                    key,
                    [false, false, false, false, false, false, true, false],
                    State::Running,
                    true,
                    true
                ),
                Some(expected)
            );
        }
        // Without the platform menu, Command+Q reaches the shell, which asks
        // before quitting while a machine runs.
        assert_eq!(
            super::hotkey(
                KeyCode::KeyQ,
                [false, false, false, false, false, false, true, false],
                State::Running,
                true,
                true
            ),
            Some(Exit)
        );
        assert_eq!(
            super::hotkey(
                KeyCode::KeyQ,
                [false, false, false, false, true, false, true, false],
                State::Running,
                true,
                true
            ),
            None
        );
    }

    #[test]
    fn fix_round_library_ignores_dotfiles_and_pending_files() {
        let temp = assembly::test_support::TempDir::new("library-pending");
        std::fs::create_dir_all(temp.path().join("local")).unwrap();
        for file in [
            "visible.toml",
            ".hidden.toml",
            "stale.pending.toml",
            "local/.settings.pending.toml",
            "local/saved.toml",
        ] {
            std::fs::write(temp.path().join(file), "").unwrap();
        }
        let paths = machine_paths(temp.path()).unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|path| path
            .file_name()
            .is_some_and(|name| name == "visible.toml" || name == "saved.toml")));
    }

    #[test]
    fn row_warnings_exclude_pending_checks_and_skipped_roots() {
        let mut entry = LibraryEntry::pending("synthetic.toml".into());
        assert!(!entry.has_problem());
        assert!(!entry.hint().contains("Required firmware"));
        entry.checked = true;
        entry.warning = None;
        entry
            .discovery_warnings
            .push("skipped unreadable root".into());
        assert!(!entry.has_problem());
        entry.warning = Some("missing local disk".into());
        assert!(entry.has_problem());
    }

    #[test]
    fn cause_specific_hints_distinguish_media_firmware_and_integrity() {
        let mut entry = LibraryEntry::pending("machines/example.toml".into());
        entry.checked = true;
        for (warning, expected) in [
            (
                "card[ide].drives[master]: cannot read image local/games.img",
                "Create blank disk",
            ),
            (
                "card[fdc].drives[unit=0]: cannot read image local/transfer-1200.img",
                "Create blank disk",
            ),
            (
                "card[fdc].drives[unit=0]: SHA-256 mismatch",
                "Restore the expected image",
            ),
            ("firmware.bios: missing ROM", "Locate firmware"),
            ("memory: invalid size", "configuration field"),
        ] {
            entry.warning = Some(warning.into());
            assert!(entry.hint().contains(expected), "{}", entry.hint());
        }
        assert_eq!(
            relative_diagnostic(
                "machines/local/games.img",
                Path::new("machines/example.toml")
            ),
            "local/games.img"
        );
    }

    #[test]
    fn status_cells_fit_without_consuming_each_others_width() {
        assert_eq!(
            status_widths(1040.0),
            [240.0, 68.0, 298.0, 100.0, 92.0, 118.0, 124.0]
        );
        assert_eq!(
            status_widths(760.0),
            [178.0, 68.0, 80.0, 100.0, 92.0, 118.0, 124.0]
        );
        assert_eq!(status_widths(4000.0)[2], 900.0);
        for available in [0.0, 200.0, 760.0, 1040.0, 1300.0, 2000.0, 4000.0] {
            let widths = status_widths(available);
            assert!(widths.iter().all(|width| *width >= 0.0));
            assert!((widths.iter().sum::<f32>() - available).abs() < 0.01);
            if available >= 760.0 {
                // Power, CPU, EMS, audio and speed never change width.
                for cell in [1, 3, 4, 5, 6] {
                    assert_eq!(widths[cell], status_widths(760.0)[cell]);
                }
                assert!((80.0..=900.0).contains(&widths[2]));
                assert!(widths[0] >= 178.0);
            }
        }
    }

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
            LibraryCheck {
                path,
                options: assembly::AssemblyOptions::default(),
                roots: vec![],
                cache: None,
                store_root: None,
                rescan: false,
                refresh_listing: false,
                control: Default::default(),
                session: Default::default(),
                prepared: None,
            },
            move || {
                tx.send(std::thread::current().id()).unwrap();
            },
        )
        .recv_timeout(crate::test_wait::HANG_GUARD)
        .unwrap();
        assert_ne!(
            rx.recv_timeout(crate::test_wait::HANG_GUARD).unwrap(),
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
        assert_eq!(
            select_machine(&mut paths, Path::new("first.toml"), true),
            Some(0)
        );
        assert_eq!(paths.len(), 1);
        let selected = select_machine(&mut paths, Path::new("missing.toml"), true).unwrap();
        assert_eq!(paths[selected], Path::new("missing.toml"));
    }

    /// A fresh install has no default machine file: the library starts with
    /// nothing selected instead of a row for a file that does not exist.
    #[test]
    fn a_missing_default_machine_is_neither_listed_nor_selected() {
        let temp = assembly::test_support::TempDir::new("default-machine-selection");
        let mut paths = vec![PathBuf::from("first.toml")];
        let missing = temp.path().join("at-freedos.toml");
        assert_eq!(select_machine(&mut paths, &missing, false), None);
        assert_eq!(paths, [PathBuf::from("first.toml")]);
        assert_eq!(select_machine(&mut paths, Path::new(""), false), None);
        assert_eq!(paths.len(), 1);
        std::fs::write(&missing, "schema=1\n").unwrap();
        assert_eq!(select_machine(&mut paths, &missing, false), Some(1));
        assert_eq!(paths[1], missing);
        assert_eq!(select_machine(&mut paths, &missing, false), Some(1));
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
                    Command::InsertSet(0),
                    Command::WriteMode(0),
                    Command::Next(0),
                    Command::Mute,
                ] {
                    assert_eq!(model.allows(command), state != State::Stopped);
                }
                // A stopped machine's drives take floppies for Start.
                for command in [
                    Command::Insert(0),
                    Command::InsertFile(0),
                    Command::Eject(0),
                ] {
                    assert!(model.allows(command), "{state:?} {command:?}");
                }
                assert!(!model.allows(Command::Eject(1)));
                assert!(!model.allows(Command::Insert(1)));
                assert!(!model.allows(Command::InsertFile(1)));
                assert!(!model.allows(Command::WriteMode(1)));
                assert!(!model.allows(Command::Insert(2)));
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
        assert_eq!(hotkey(Space, true, true, false, State::Running, true), None);
        assert_eq!(
            hotkey(KeyT, false, false, false, State::Running, true),
            None
        );
        for (key, command) in [
            (KeyI, Command::Insert(0)),
            (KeyF, Command::Next(0)),
            (KeyE, Command::Eject(0)),
            (KeyM, Command::Mute),
            (KeyG, Command::Release),
            (KeyP, Command::Pause),
            (KeyT, Command::FastForward),
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
    /// F2, Ctrl+D (⌘D), Delete (⌘⌫) and Ctrl+Shift+N (⌘⇧N) act outside guest
    /// focus, stopped or not; with the guest focused they stay guest input.
    #[test]
    fn library_keys_act_outside_guest_focus() {
        use KeyCode::*;
        let none = [false; 8];
        let ctrl = [true, false, false, false, false, false, false, false];
        let ctrl_shift = [true, false, false, false, true, false, false, false];
        let cmd = [false, false, false, false, false, false, true, false];
        let cmd_shift = [false, false, false, false, true, false, true, false];
        for state in [State::Stopped, State::Running, State::Paused] {
            for (key, modifiers, macos, command) in [
                (F2, none, false, Command::Rename),
                (F2, none, true, Command::Rename),
                (KeyD, ctrl, false, Command::Duplicate),
                (KeyD, cmd, true, Command::Duplicate),
                (Delete, none, false, Command::DeleteSelection),
                (Backspace, cmd, true, Command::DeleteSelection),
                (KeyN, ctrl_shift, false, Command::NewFromImage),
                (KeyN, cmd_shift, true, Command::NewFromImage),
            ] {
                assert_eq!(
                    super::hotkey(key, modifiers, state, false, macos),
                    Some(command),
                    "{key:?} {state:?} macos={macos}"
                );
                let focused = super::hotkey(key, modifiers, state, true, macos);
                if state == State::Stopped {
                    assert_eq!(focused, Some(command), "{key:?} stopped");
                } else {
                    assert_eq!(focused, None, "{key:?} {state:?} reaches the guest");
                }
            }
        }
        // Delete alone on macOS and Shift+Delete elsewhere are no shortcut.
        assert_eq!(
            super::hotkey(Delete, none, State::Stopped, false, true),
            None
        );
        let shift = [false, false, false, false, true, false, false, false];
        assert_eq!(
            super::hotkey(Delete, shift, State::Stopped, false, false),
            None
        );
        assert!(Command::DeleteSelection.library_key());
        assert!(!Command::Stop.library_key());
        // Whether they apply then is their enablement's call.
        let running = Enablement {
            state: State::Running,
            ready: true,
            floppy: [true, false],
            audio: false,
        };
        for command in [
            Command::Duplicate,
            Command::DeleteMachine,
            Command::DeleteSelection,
            Command::ResetMachine,
            Command::NewFromImage,
            Command::ImageStart,
            Command::ImageGaming,
        ] {
            assert!(!running.allows(command), "{command:?}");
            assert!(
                Enablement {
                    state: State::Stopped,
                    ..running
                }
                .allows(command),
                "{command:?}"
            );
        }
        assert!(running.allows(Command::Rename));
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
        assert_eq!(entry.ems, "EMS unknown");
    }
    #[test]
    fn missing_local_media_names_the_media_cause_and_shortens_resolved_paths() {
        let temp = assembly::test_support::TempDir::new("missing-media-hint");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let path = temp.path().join("machines/example.toml");
        std::fs::write(temp.path().join("machines/bios.bin"), vec![0xF4; 32768]).unwrap();
        std::fs::write(&path, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.2M'\nimage='local/transfer-1200.img'\nwrite='readonly'\n").unwrap();
        let entry = LibraryEntry::inspect(
            path,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        );
        assert!(entry
            .warning
            .as_deref()
            .unwrap()
            .contains("transfer-1200.img"));
        assert!(!entry
            .warning
            .as_deref()
            .unwrap()
            .contains(&temp.path().to_string_lossy().to_string()));
        assert!(
            entry.hint().contains("Create blank disk"),
            "{}: {}",
            entry.warning.as_deref().unwrap(),
            entry.hint()
        );
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
                    (KeyCode::Comma, true, Command::Settings),
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
