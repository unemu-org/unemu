// SPDX-License-Identifier: AGPL-3.0-or-later
//! Cut, copy and paste shortcuts for the shell's own text fields.
//!
//! The window keeps egui's built-in clipboard shortcuts off (eframe
//! `NativeOptions::clipboard_shortcuts`). With them on, egui-winit reads the
//! host clipboard on every paste chord the window receives, whether or not a
//! text field could take the text. The guest does not depend on that setting:
//! while the emulated machine has the keyboard, the window queues its keys for
//! the guest and never hands them to egui, so Ctrl+C stays the guest's (DOS
//! break) on every host. See `backlog/docs/architecture.md` § Observation and presentation.
//!
//! [`shortcuts`] gives the chords back to the focused text field only. It
//! maps a key press as egui-winit maps it with its shortcuts on: the dedicated
//! Cut, Copy and Paste keys, X, C and V under the command modifier alone
//! (Command on macOS, Ctrl elsewhere), and on Windows also Shift+Delete,
//! Ctrl+Insert and Shift+Insert.
//!
//! Paste reads the host clipboard at once, through the shell's
//! [`HostClipboard`], and puts egui's Paste event where the chord was, so the
//! keys around it keep their order: a password pasted and confirmed with
//! Enter in one pass is the one submitted, and Tab after a paste moves on only
//! once the text is in. In the program that clipboard is [`SystemClipboard`],
//! one arboard clipboard opened at the first paste and kept. egui-winit keeps
//! its own clipboard inside eframe, out of the application's reach, so a
//! synchronous read needs one of the shell's own.
//!
//! Cut and copy become egui's Cut and Copy events. The field then writes its
//! selection through egui's copy output, which egui-winit hands to the host
//! clipboard (arboard, and smithay-clipboard on Wayland); without eframe's
//! `clipboard` feature that clipboard would be a string inside the program.
//!
//! A field marked with [`mark_password`] takes paste only. egui already keeps
//! a password off the clipboard on copy, but its cut would still delete the
//! selection, so both chords do nothing there.

use egui::{Event, Key, Modifiers};

/// The host clipboard as a paste reads it.
pub trait HostClipboard {
    /// The clipboard's text; `None` when it holds no text or cannot be read.
    fn text(&mut self) -> Option<String>;
}

/// The operating system's clipboard through arboard: the general pasteboard
/// on macOS, the Windows clipboard, and on Linux the Wayland data-control
/// protocol when the compositor offers it, otherwise X11. Opened at the first
/// paste, so no paste means no clipboard access, and kept from then on; a host
/// without a clipboard service pastes nothing.
#[derive(Default)]
pub struct SystemClipboard {
    clipboard: Option<arboard::Clipboard>,
}

impl HostClipboard for SystemClipboard {
    fn text(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()?.get_text().ok()
    }
}

/// What one key press asks of the focused text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shortcut {
    Cut,
    Copy,
    Paste,
}

/// The data marker of a password field.
#[derive(Clone, Copy)]
struct PasswordField;

/// Marks the text field `id` as a password field: it takes paste, never cut
/// or copy.
pub fn mark_password(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|data| data.insert_temp(id, PasswordField));
}

/// Gives the platform's cut, copy and paste chords to the focused text field.
/// Called once a pass before any field draws; while no text field has the
/// keyboard it changes nothing and leaves `clipboard` alone.
pub fn shortcuts(ctx: &egui::Context, clipboard: &mut dyn HostClipboard) {
    if !ctx.text_edit_focused() {
        return;
    }
    let password = ctx
        .memory(|memory| memory.focused())
        .is_some_and(|id| ctx.data(|data| data.get_temp::<PasswordField>(id).is_some()));
    let windows = cfg!(target_os = "windows");
    let paste = ctx.input(|input| {
        input
            .events
            .iter()
            .any(|event| chord(event, windows) == Some(Shortcut::Paste))
    });
    // egui-winit's own paste makes Windows line ends Unix ones and drops an
    // empty paste.
    let text = paste
        .then(|| clipboard.text())
        .flatten()
        .map(|text| text.replace("\r\n", "\n"))
        .filter(|text| !text.is_empty());
    ctx.input_mut(|input| translate(&mut input.events, password, windows, text.as_deref()));
}

/// Turns the clipboard chords among `events` into egui's clipboard events in
/// place. A paste chord becomes `text`, or leaves the list without one.
fn translate(events: &mut Vec<Event>, password: bool, windows: bool, text: Option<&str>) {
    events.retain_mut(|event| match chord(event, windows) {
        None => true,
        Some(Shortcut::Paste) => match text {
            Some(text) => {
                *event = Event::Paste(text.to_owned());
                true
            }
            None => false,
        },
        Some(_) if password => false,
        Some(Shortcut::Copy) => {
            *event = Event::Copy;
            true
        }
        Some(Shortcut::Cut) => {
            *event = Event::Cut;
            true
        }
    });
}

fn chord(event: &Event, windows: bool) -> Option<Shortcut> {
    match event {
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => shortcut(*key, *modifiers, windows),
        _ => None,
    }
}

fn shortcut(key: Key, modifiers: Modifiers, windows: bool) -> Option<Shortcut> {
    let command = modifiers.command_only();
    match key {
        Key::Cut => Some(Shortcut::Cut),
        Key::Copy => Some(Shortcut::Copy),
        Key::Paste => Some(Shortcut::Paste),
        Key::X if command => Some(Shortcut::Cut),
        Key::C if command => Some(Shortcut::Copy),
        Key::V if command => Some(Shortcut::Paste),
        Key::Delete if windows && modifiers.shift => Some(Shortcut::Cut),
        Key::Insert if windows && modifiers.ctrl => Some(Shortcut::Copy),
        Key::Insert if windows && modifiers.shift => Some(Shortcut::Paste),
        _ => None,
    }
}

/// A fake host clipboard and a headless egui host, for the tests of the
/// windows that use these shortcuts.
#[cfg(any(test, feature = "test-support"))]
pub mod fake {
    use super::HostClipboard;
    use egui::{Event, Key, Modifiers};
    use std::{cell::RefCell, rc::Rc};

    /// The command modifier as egui-winit reports it on macOS (Command).
    pub const MAC_COMMAND: Modifiers = Modifiers {
        mac_cmd: true,
        command: true,
        ..Modifiers::NONE
    };
    /// The command modifier as egui-winit reports it elsewhere (Ctrl).
    pub const CTRL_COMMAND: Modifiers = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };

    pub fn press(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    pub fn release(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers,
        }
    }

    /// What the fake host clipboard holds and how often it was used.
    #[derive(Default)]
    pub struct Fake {
        pub text: String,
        pub reads: usize,
        pub writes: usize,
    }

    /// A fake host clipboard; clones share one [`Fake`].
    #[derive(Clone, Default)]
    pub struct FakeClipboard(pub Rc<RefCell<Fake>>);

    impl HostClipboard for FakeClipboard {
        fn text(&mut self) -> Option<String> {
            let mut fake = self.0.borrow_mut();
            fake.reads += 1;
            Some(fake.text.clone())
        }
    }

    /// Runs headless passes and plays egui-winit's part for copy: a copy
    /// output writes the fake clipboard.
    pub struct Host {
        pub ctx: egui::Context,
        pub clipboard: FakeClipboard,
    }

    impl Host {
        pub fn new(text: &str) -> Self {
            let clipboard = FakeClipboard::default();
            clipboard.0.borrow_mut().text = text.into();
            Self {
                ctx: egui::Context::default(),
                clipboard,
            }
        }

        pub fn set_text(&self, text: &str) {
            self.clipboard.0.borrow_mut().text = text.into();
        }

        pub fn text(&self) -> String {
            self.clipboard.0.borrow().text.clone()
        }

        pub fn reads(&self) -> usize {
            self.clipboard.0.borrow().reads
        }

        pub fn writes(&self) -> usize {
            self.clipboard.0.borrow().writes
        }

        /// Runs one pass with `events` and returns the painted text with its
        /// screen rectangle.
        pub fn pass(
            &mut self,
            events: Vec<Event>,
            mut run: impl FnMut(&mut egui::Ui),
        ) -> Vec<(String, egui::Rect)> {
            fn texts(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
                match shape {
                    egui::Shape::Text(text) => {
                        out.push((text.galley.text().to_owned(), text.visual_bounding_rect()))
                    }
                    egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| texts(shape, out)),
                    _ => {}
                }
            }
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut output = self.ctx.run_ui(input, |ui| run(ui));
            output.textures_delta.clear();
            for command in &output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    let mut fake = self.clipboard.0.borrow_mut();
                    fake.writes += 1;
                    fake.text = text.clone();
                }
            }
            let mut painted = Vec::new();
            for shape in &output.shapes {
                texts(&shape.shape, &mut painted);
            }
            output.drop_without_applying_deltas();
            painted
        }

        /// Clicks the primary button at `at` over two passes.
        pub fn click(&mut self, at: egui::Pos2, mut run: impl FnMut(&mut egui::Ui)) {
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            self.pass(vec![Event::PointerMoved(at), button(true)], &mut run);
            self.pass(vec![button(false)], &mut run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[derive(Default)]
    struct Fields {
        plain: String,
        secret: String,
        /// The plain field's text when it lost the keyboard to Enter, as a
        /// dialog that submits on Enter reads it.
        entered: Option<String>,
    }

    const PLAIN: &str = "plain-field";
    const SECRET: &str = "secret-field";

    fn draw(ui: &mut egui::Ui, fields: &mut Fields, clipboard: &mut FakeClipboard) {
        shortcuts(ui.ctx(), clipboard);
        let plain =
            ui.add(egui::TextEdit::singleline(&mut fields.plain).id(egui::Id::unique(PLAIN)));
        if plain.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            fields.entered = Some(fields.plain.clone());
        }
        let secret = ui.add(
            egui::TextEdit::singleline(&mut fields.secret)
                .id(egui::Id::unique(SECRET))
                .password(true),
        );
        mark_password(ui.ctx(), secret.id);
    }

    fn focus(host: &Host, id: &str) {
        host.ctx
            .memory_mut(|memory| memory.request_focus(egui::Id::unique(id)));
    }

    /// Selects the whole focused field with the platform's select-all chord.
    fn select_all(modifiers: Modifiers) -> Event {
        press(Key::A, modifiers)
    }

    #[test]
    fn both_shortcut_styles_and_the_dedicated_keys_map_as_egui_winit_does() {
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            assert_eq!(shortcut(Key::X, modifiers, false), Some(Shortcut::Cut));
            assert_eq!(shortcut(Key::C, modifiers, false), Some(Shortcut::Copy));
            assert_eq!(shortcut(Key::V, modifiers, false), Some(Shortcut::Paste));
            for extra in [Modifiers::SHIFT, Modifiers::ALT] {
                assert_eq!(shortcut(Key::V, modifiers | extra, false), None);
            }
        }
        // Ctrl on macOS is not the command modifier: Ctrl+C there is a key.
        assert_eq!(shortcut(Key::C, Modifiers::CTRL, false), None);
        assert_eq!(shortcut(Key::V, Modifiers::NONE, false), None);
        assert_eq!(
            shortcut(Key::Cut, Modifiers::NONE, false),
            Some(Shortcut::Cut)
        );
        assert_eq!(
            shortcut(Key::Copy, Modifiers::NONE, false),
            Some(Shortcut::Copy)
        );
        assert_eq!(
            shortcut(Key::Paste, Modifiers::NONE, false),
            Some(Shortcut::Paste)
        );
        for windows in [false, true] {
            let expect = |shortcut| windows.then_some(shortcut);
            assert_eq!(
                shortcut(Key::Delete, Modifiers::SHIFT, windows),
                expect(Shortcut::Cut)
            );
            assert_eq!(
                shortcut(Key::Insert, Modifiers::CTRL, windows),
                expect(Shortcut::Copy)
            );
            assert_eq!(
                shortcut(Key::Insert, Modifiers::SHIFT, windows),
                expect(Shortcut::Paste)
            );
        }
    }

    #[test]
    fn an_ordinary_field_cuts_copies_and_pastes_in_both_shortcut_styles() {
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            let mut fields = Fields {
                plain: "copied".into(),
                ..Fields::default()
            };
            let mut host = Host::new("ignored");
            let mut clipboard = host.clipboard.clone();
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            focus(&host, PLAIN);
            host.pass(vec![select_all(modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            host.pass(vec![press(Key::C, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            assert_eq!((host.text().as_str(), host.writes()), ("copied", 1));
            assert_eq!(fields.plain, "copied", "copy leaves the text");

            host.pass(vec![press(Key::X, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            assert_eq!((host.text().as_str(), host.writes()), ("copied", 2));
            assert_eq!(fields.plain, "", "cut takes the selection");

            host.set_text("pasted\r\ntext");
            host.pass(
                vec![press(Key::V, modifiers), release(Key::V, modifiers)],
                |ui| draw(ui, &mut fields, &mut clipboard),
            );
            assert_eq!(fields.plain, "pasted text", "a single line takes a space");
            assert_eq!(host.reads(), 1, "one read per paste");

            // An empty clipboard pastes nothing, and the chord is gone.
            host.set_text("");
            let mut seen = Vec::new();
            host.pass(vec![press(Key::V, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard);
                seen = ui.input(|input| input.events.clone());
            });
            assert_eq!(fields.plain, "pasted text");
            assert!(seen.is_empty(), "{seen:?}");
        }
    }

    #[test]
    fn a_paste_keeps_its_place_among_the_keys_of_its_pass() {
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            // Enter after the paste submits the pasted text.
            let mut fields = Fields::default();
            let mut host = Host::new("pasted");
            let mut clipboard = host.clipboard.clone();
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            focus(&host, PLAIN);
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            host.pass(
                vec![press(Key::V, modifiers), press(Key::Enter, Modifiers::NONE)],
                |ui| draw(ui, &mut fields, &mut clipboard),
            );
            assert_eq!(fields.entered.as_deref(), Some("pasted"));

            // Tab after the paste moves on with the text in the first field.
            let mut fields = Fields::default();
            let mut host = Host::new("pasted");
            let mut clipboard = host.clipboard.clone();
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            focus(&host, PLAIN);
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            host.pass(
                vec![press(Key::V, modifiers), press(Key::Tab, Modifiers::NONE)],
                |ui| draw(ui, &mut fields, &mut clipboard),
            );
            for _ in 0..2 {
                host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            }
            assert_eq!(
                host.ctx.memory(|memory| memory.focused()),
                Some(egui::Id::unique(SECRET)),
                "Tab moved on"
            );
            assert_eq!(
                (fields.plain.as_str(), fields.secret.as_str()),
                ("pasted", "")
            );
        }
    }

    #[test]
    fn a_password_field_takes_paste_and_refuses_copy_and_cut() {
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            let mut fields = Fields::default();
            let mut host = Host::new("synthetic-secret");
            let mut clipboard = host.clipboard.clone();
            host.pass(Vec::new(), |ui| draw(ui, &mut fields, &mut clipboard));
            focus(&host, SECRET);
            host.pass(vec![press(Key::V, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            assert_eq!(fields.secret, "synthetic-secret");
            assert_eq!(host.reads(), 1);

            host.set_text("unchanged");
            host.pass(vec![select_all(modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            host.pass(vec![press(Key::C, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            host.pass(vec![press(Key::X, modifiers)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            host.pass(vec![press(Key::Cut, Modifiers::NONE)], |ui| {
                draw(ui, &mut fields, &mut clipboard)
            });
            assert_eq!(fields.secret, "synthetic-secret", "cut deletes nothing");
            assert_eq!((host.text().as_str(), host.writes()), ("unchanged", 0));
        }
    }

    #[test]
    fn without_a_focused_text_field_no_chord_changes_and_no_clipboard_is_read() {
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            let mut fields = Fields {
                plain: "kept".into(),
                ..Fields::default()
            };
            let mut host = Host::new("never read");
            let mut clipboard = host.clipboard.clone();
            let chords = || {
                [
                    (Key::C, modifiers),
                    (Key::V, modifiers),
                    (Key::X, modifiers),
                ]
                .into_iter()
                .chain([(Key::Paste, Modifiers::NONE)])
                .flat_map(|(key, modifiers)| [press(key, modifiers), release(key, modifiers)])
                .collect::<Vec<_>>()
            };
            let mut seen = Vec::new();
            host.pass(chords(), |ui| {
                draw(ui, &mut fields, &mut clipboard);
                seen = ui.input(|input| input.events.clone());
            });
            assert_eq!(seen, chords(), "every chord stays an ordinary key");
            // A focused button is not a text field.
            let mut button = None;
            host.pass(Vec::new(), |ui| {
                draw(ui, &mut fields, &mut clipboard);
                let response = ui.button("Button");
                response.request_focus();
                button = Some(response.id);
            });
            host.pass(chords(), |ui| {
                assert_eq!(ui.ctx().memory(|memory| memory.focused()), button);
                draw(ui, &mut fields, &mut clipboard);
                seen = ui.input(|input| input.events.clone());
                let _ = ui.button("Button");
            });
            assert_eq!(host.ctx.memory(|memory| memory.focused()), button);
            assert_eq!(seen, chords());
            assert_eq!((host.reads(), host.writes()), (0, 0));
            assert_eq!(host.text(), "never read");
            assert_eq!(fields.plain, "kept");
        }
    }

    /// The program's clipboards reach the host. Copy and cut go through
    /// egui-winit, which has a host clipboard only with eframe's `clipboard`
    /// feature; paste opens arboard. Opening reads and writes nothing. macOS
    /// and Windows always have a clipboard service, so arboard must open
    /// there, also headless. Linux and the BSDs have one only with an X11 or
    /// Wayland display: without `DISPLAY` and `WAYLAND_DISPLAY`, as on a
    /// headless CI runner, the open is skipped and only the build check runs.
    #[test]
    fn the_program_builds_and_opens_a_host_clipboard() {
        let manifest: toml::Table = toml::from_str(include_str!("../../../Cargo.toml")).unwrap();
        let features = manifest["workspace"]["dependencies"]["eframe"]["features"]
            .as_array()
            .unwrap();
        assert!(
            features
                .iter()
                .any(|feature| feature.as_str() == Some("clipboard")),
            "eframe without `clipboard` keeps copy and cut inside the program: {features:?}"
        );
        let display =
            std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
        if cfg!(all(unix, not(target_os = "macos"))) && !display {
            eprintln!("no X11 or Wayland display: the host clipboard is not opened");
            return;
        }
        if let Err(error) = arboard::Clipboard::new() {
            panic!("the host clipboard did not open: {error}");
        }
    }
}
