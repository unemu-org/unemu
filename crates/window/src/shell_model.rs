// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure desktop hotkeys, the status bar's widths and texture conversion.
//!
//! The command vocabulary is `pc_ui::commands` and the library's machine
//! files are `pc_library::machine_files`; both are re-exported here under
//! the paths the desktop names them by. Presentation consumes decoded
//! frames; see `backlog/docs/design/machine-images.md` §7 and
//! `backlog/docs/architecture.md` § Observation and presentation.

pub(crate) use pc_library::machine_files::{
    machine_paths, select_machine, spawn_library_check_with_compatibility, LibraryCheck,
    LibraryEntry, PreparedEntry,
};
pub(crate) use pc_ui::commands::{Command, Enablement, State};
use winit::keyboard::{Key, KeyCode, NamedKey};

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
            "s" => Some(KeyS),
            "l" => Some(KeyL),
            "n" => Some(KeyN),
            "o" => Some(KeyO),
            "," => Some(Comma),
            "/" | "?" => Some(Slash),
            _ => None,
        },
        Key::Named(NamedKey::End) => Some(End),
        Key::Named(NamedKey::Enter) => Some(Enter),
        Key::Named(NamedKey::F2) => Some(F2),
        Key::Named(NamedKey::F5) => Some(F5),
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
            (KeyS, false) => Some(SaveState),
            (KeyL, false) => Some(LoadState),
            (End, false) => Some(Reset),
            (Enter, false) => Some(Fullscreen),
            (KeyN, false) => Some(Placeholder("New machine")),
            (KeyO, false) => Some(Command::Open),
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
            KeyS if !shift => SaveState,
            KeyL if !shift => LoadState,
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
            KeyO => Some(Command::Open),
            Comma => Some(Settings),
            _ => None,
        };
    }
    let plain = !alt && !ctrl && !shift && !command;
    match key {
        F1 if !macos => Some(Shortcuts),
        Enter if state == State::Stopped && !alt && !ctrl => Some(Start),
        F2 if plain => Some(Rename),
        F5 if plain && state == State::Stopped => Some(RefreshRemote),
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
        "Ctrl+Alt+S" => "⌘S",
        "Ctrl+Alt+L" => "⌘L",
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
        "Ctrl+Alt+Shift+D" => "⌘⇧D",
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

pub(crate) fn integer_size(
    available: egui::Vec2,
    texture: egui::Vec2,
    sink_scale: u16,
) -> egui::Vec2 {
    let original = texture / f32::from(sink_scale);
    let fit = (available.x / original.x).min(available.y / original.y);
    original * if fit >= 1.0 { fit.floor() } else { fit }
}

pub(crate) fn texture_image(frame: &machine_view::Picture) -> egui::ColorImage {
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
            ("s", false, SaveState),
            ("l", false, LoadState),
            ("n", false, Placeholder("New machine")),
            ("o", false, Open),
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
        let image = texture_image(&machine_view::Picture {
            width: 2,
            height: 2,
            pixels: vec![0xff0000, 0x00ff00, 0x0000ff, 0x123456],
            aspect: (4, 3),
            serial: 0,
            visible: unemu_core_api::FrameArea::new(0, 0, 2, 2),
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
    fn ordinary_shortcuts_belong_to_a_focused_guest() {
        for state in [State::Stopped, State::Running, State::Paused] {
            for focus in [true, false] {
                for (key, ctrl, command) in [
                    (KeyCode::F1, false, Command::Shortcuts),
                    (KeyCode::KeyN, true, Command::Placeholder("New machine")),
                    (KeyCode::KeyO, true, Command::Open),
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
