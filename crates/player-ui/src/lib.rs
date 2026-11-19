// SPDX-License-Identifier: AGPL-3.0-or-later
//! The view of one machine started from a file, driven through the core
//! contract alone (`backlog/docs/design/core-contract.md` §13.3, "The front
//! end's generic launch path", and the approved mock-up
//! `backlog/docs/design/mockups/player.html`).
//!
//! A [`Player`] starts from a [`core_launch::Prepared`] file and holds the
//! running `dyn Machine` in a [`machine_view::Session`]. Everything it shows
//! comes from the machine's description, never from trying a call
//! (core-contract.md §3.3):
//!
//! | Control | Offered when | Does |
//! |---|---|---|
//! | Pause, Resume | always | stops advancing and anchors pacing again on resume |
//! | Reset | the machine lists a soft reset | a soft reset; a refusal is a status-line message |
//! | Power cycle | always | a hard reset when listed, otherwise the machine finishes and starts again from the same file |
//! | Stop | always | the machine finishes; the application returns to its library |
//! | Mute, Speed, Fast forward, Full screen | always | as for every machine |
//! | The Media menu | a removable slot | one section per removable slot: Eject, Next in catalogue and the catalogue's entries |
//! | Input… | a pad port | the Input dialog over [`machine_view::pads::PadBindings`] |
//! | Show overscan | the picture's visible part is smaller than the picture | a view option, off by default |
//! | Pointer capture | a relative pointer | as for every machine |
//! | Aiming | an absolute pointer on the shown display | the pointer is not captured; over the screen it is a cross-hair |
//!
//! A control the description does not offer is absent, not greyed. Saved
//! state comes with the quick slot, later; nothing here saves or loads one.
//!
//! The application keeps its own menus (File, Edit, Image, Tools, Help), its
//! toolbar and its hotkeys, and hands the player what meets a running
//! machine: [`Player::menus`] draws the Machine, Media and View menus inside
//! its menu bar, [`Player::screen`] the picture in its central panel,
//! [`Player::status`] the status line, [`Player::dialogs`] the player's own
//! dialogs; [`Player::command`] carries out the player's [`Command`]s, from
//! those menus and from the application's hotkeys and toolbar;
//! [`Player::tick`] advances the machine; raw key, button and pointer events
//! go to [`Player::key_event`] and its siblings. The player's command
//! vocabulary is its own: none of it is the shell's.
//!
//! The picture keeps the shape the machine states: [`machine_view::screen`]
//! does the geometry, with overscan hidden by default and integer scaling as
//! the View menu chooses. The crate knows no platform and names no emulator
//! crate, in its tests either, which run against `unemu-test-core`; the
//! boundary check keeps it so (layer `neutral`, a window crate,
//! `scripts/crate-boundaries.sh`).

mod commands;
mod input_dialog;
mod menus;
mod message;
mod screen;
mod status;

pub use crate::commands::Command;
pub use crate::input_dialog::key_label;
pub use crate::menus::ViewChrome;
pub use crate::message::MessageBox;

use core_launch::Prepared;
use machine_view::{pacing::Speed, pads::PadBindings, Session};
use std::{
    error::Error,
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};
use unemu_core_api::{
    DisplayId, InputKind, MachineDescription, MediaAction, PadControl, PortId, ResetKind, SlotId,
    SlotState,
};
use winit::{
    event::{ElementState, MouseButton},
    keyboard::KeyCode,
    window::Window,
};

/// What the application does after a command or a dialog of the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The machine finished: drop the player and show the library. A
    /// failure comes along: one to finish cleanly, for the application to
    /// show, or the guest's failure that ended the run, which the player's
    /// own box has already shown.
    Stopped { failure: Option<String> },
    /// Enter or leave full screen.
    Fullscreen,
    /// The Input dialog's OK changed the pad keys: store
    /// [`Player::pad_bindings`] with the preferences.
    PadBindings,
}

/// The controls of the "What a running machine offers" table that depend on
/// the machine's description; the others are always there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offers {
    /// Machine › Reset: the machine lists a soft reset.
    pub reset: bool,
    /// Power cycle is a hard reset: the machine lists one. Without it,
    /// Power cycle finishes the machine and starts it again.
    pub power_cycle_by_reset: bool,
    /// The Media menu's sections, one per removable slot, in the
    /// description's order.
    pub media: Vec<SlotId>,
    /// Machine › Input…: the machine has a pad port.
    pub input: bool,
    /// View › Show overscan: the latest picture shows less than all of it.
    pub show_overscan: bool,
    /// The pointer is captured on a click: the machine has a relative
    /// pointer.
    pub pointer_capture: bool,
    /// The pointer aims, uncaptured, with a cross-hair over the screen: the
    /// machine has an absolute pointer on the shown display.
    pub aiming: bool,
}

/// One pad port, a tab of the Input dialog.
#[derive(Clone, Debug)]
struct PadPort {
    label: String,
    controls: Vec<PadControl>,
}

/// One removable slot: a section of the Media menu and a chip of the status
/// line.
#[derive(Clone, Debug)]
struct SlotView {
    id: SlotId,
    label: String,
    /// The machine's answer at the latest boundary the player looked at.
    state: Option<SlotState>,
    /// Host time until which the chip's light stays on.
    lit_until: Option<Duration>,
}

impl SlotView {
    /// What is in the slot, by its catalogue name.
    fn inserted(&self) -> Option<&str> {
        let state = self.state.as_ref()?;
        state
            .catalogue
            .get(state.inserted?)
            .map(|medium| medium.name.as_str())
    }
}

/// Where the machine is, as the light, the message and the Machine menu's
/// note name it. Only the user pauses; the others come from the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Condition {
    Running,
    /// The user paused a running machine.
    Paused,
    /// The guest waits for a reset.
    Halted,
    /// The guest switched the machine off, the run failed, or the machine
    /// could not start again.
    Stopped,
}

impl Condition {
    /// The light's word, and the Machine menu note's in lower case.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Halted => "Halted",
            Self::Stopped => "Stopped",
        }
    }
}

/// The dialog the player shows over itself, at most one.
enum Dialog {
    Input(input_dialog::InputDialog),
    Message(MessageBox),
}

/// How long a slot's light stays on after the slot was busy, as the drive
/// lights of every machine do: long enough to see a single access. Host UI
/// time only; the machine never reads it.
const LIGHT_HOLD: Duration = Duration::from_millis(200);

/// The status line's message once the machine could not start again.
const STOPPED: &str = "Machine stopped — Power cycle starts it again, Stop returns to the library";
/// The status line's message once the guest halted, waiting for a reset.
const HALTED: &str = "Machine halted — Power cycle restarts it, Stop returns to the library";
/// The status line's message once the guest switched the machine off.
const SWITCHED_OFF: &str =
    "Machine switched off — Power cycle starts it again, Stop returns to the library";

/// One running machine started from a file, and its view.
pub struct Player {
    prepared: Prepared,
    /// `None` once a power cycle could not start the machine again.
    session: Option<Session>,
    offers: Offers,
    pads: Vec<PadPort>,
    slots: Vec<SlotView>,
    bindings: PadBindings,
    /// The user's pause, of a running machine only.
    paused: bool,
    /// The speed the user chose, kept for a machine started again.
    speed: Speed,
    /// The user's mute, to put on a machine started again once its sound
    /// plays.
    mute_pending: bool,
    /// Whether the session was told it is hidden, for the user's pause, a
    /// menu or a dialog.
    modal_paused: bool,
    show_overscan: bool,
    integer_scale: bool,
    message: String,
    dialog: Option<Dialog>,
    texture: Option<(egui::TextureHandle, u64)>,
    /// Where the latest [`Player::screen`] drew the picture.
    layout: Option<machine_view::screen::ScreenLayout>,
    started: Instant,
    /// Host time of the latest tick, for the slots' lights.
    now: Duration,
    audio_opened: bool,
    window: Option<Arc<Window>>,
    classic: bool,
    macos_shortcuts: bool,
    #[cfg(test)]
    full_slices: bool,
}

impl Player {
    /// Start the machine `prepared` describes, with `bindings` as its pad
    /// keys, on the thread that will drive it. The machine's first messages
    /// go to `warnings`. The host clock of [`Player::elapsed`] starts now.
    pub fn start(
        prepared: Prepared,
        bindings: PadBindings,
        warnings: &mut impl Write,
    ) -> Result<Self, Box<dyn Error>> {
        let session = new_session(&prepared, &bindings, Duration::ZERO, warnings)?;
        let display = session.display();
        let (offers, pads, slots) = describe(session.machine().description(), display);
        let message = match aimed_label(session.machine().description(), display) {
            Some((label, buttons)) if buttons > 0 => format!(
                "The pointer over the screen aims the {label}; the primary button is its trigger"
            ),
            Some((label, _)) => format!("The pointer over the screen aims the {label}"),
            None => format!("Opened {}", prepared.file_name()),
        };
        let mut player = Self {
            prepared,
            session: Some(session),
            offers,
            pads,
            slots,
            bindings,
            paused: false,
            speed: Speed::Normal,
            mute_pending: false,
            modal_paused: false,
            show_overscan: false,
            integer_scale: false,
            message,
            dialog: None,
            texture: None,
            layout: None,
            started: Instant::now(),
            now: Duration::ZERO,
            audio_opened: false,
            window: None,
            classic: true,
            macos_shortcuts: false,
            #[cfg(test)]
            full_slices: false,
        };
        player.refresh_slots();
        Ok(player)
    }

    /// Open the host's sound output, once, as the application does for every
    /// machine it starts. A machine started again by Power cycle opens it
    /// again. Without this call the player plays no sound, as in tests.
    pub fn open_audio(&mut self, warnings: &mut impl Write) {
        self.audio_opened = true;
        if let Some(session) = &mut self.session {
            session.audio.open(warnings);
        }
    }

    /// The window the session grabs and hides the pointer in, for pointer
    /// capture; kept for a machine started again.
    pub fn set_window(&mut self, window: Option<Arc<Window>>) {
        if let Some(session) = &mut self.session {
            session.window = window.clone();
        }
        self.window = window;
    }

    /// The skin the dialogs follow (the classic title strips), and whether
    /// shortcuts read as macOS writes them.
    pub fn set_look(&mut self, classic: bool, macos_shortcuts: bool) {
        self.classic = classic;
        self.macos_shortcuts = macos_shortcuts;
    }

    /// Host time since the player started: what [`Player::tick`] and
    /// [`Player::command`] take as `elapsed`.
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// The window's title: the content, its platform, then the program's
    /// name, as the mock-up writes it.
    pub fn title(&self, program: &str) -> String {
        format!(
            "{} — {} — {program}",
            self.prepared.title(),
            self.prepared.platform_name()
        )
    }

    /// The file the player started from.
    pub fn prepared(&self) -> &Prepared {
        &self.prepared
    }

    /// The controls the machine's description offers now.
    pub fn offers(&self) -> Offers {
        let mut offers = self.offers.clone();
        offers.show_overscan = self
            .session
            .as_ref()
            .and_then(Session::picture)
            .is_some_and(screen::cropped);
        offers
    }

    /// Whether the user paused the machine.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Whether the machine runs: it advances whenever the user's pause, a
    /// menu or a dialog does not hold it. A halted, switched-off or failed
    /// machine does not run, nor one that could not start again.
    pub fn running(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.running && session.error.is_none() && !session.host_failed
        })
    }

    /// Where the machine is: running, paused by the user, halted or
    /// stopped.
    pub(crate) fn condition(&self) -> Condition {
        match &self.session {
            _ if self.running() && self.paused => Condition::Paused,
            _ if self.running() => Condition::Running,
            Some(session) if session.awaits_reset() => Condition::Halted,
            _ => Condition::Stopped,
        }
    }

    /// The speed the user chose in View › Speed, kept across a power cycle.
    pub fn selected_speed(&self) -> Speed {
        self.speed
    }

    /// The status line's message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The pad keys the session uses now.
    pub fn pad_bindings(&self) -> &PadBindings {
        &self.bindings
    }

    /// Whether one of the player's dialogs is open: the application then
    /// routes no key to the guest and treats the machine as paused.
    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Whether a click on the screen has captured the pointer.
    pub fn pointer_captured(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.mouse.captured())
    }

    /// Where the latest [`Player::screen`] drew the picture, in the window's
    /// logical points, for the application's routing of the pointer.
    pub fn screen_rect(&self) -> Option<egui::Rect> {
        self.layout.map(|layout| screen::to_egui(layout.dest))
    }

    /// Advance the machine to host time `elapsed`, unless it is paused, a
    /// menu is open (`blocked`) or a dialog shows. The machine's messages go
    /// to `warnings`. A machine that stops of itself pauses the player with
    /// a status-line message; one that fails shows a message box.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        blocked: bool,
        elapsed: Duration,
        warnings: &mut impl Write,
    ) {
        self.now = elapsed;
        let pause = self.paused || blocked || self.dialog.is_some();
        let applied = match &mut self.session {
            Some(session) if session.takes_media() => session.apply_media_events(),
            _ => Ok(()),
        };
        if let Err(error) = applied {
            self.host_failure(error.to_string());
            return;
        }
        // Also while the machine does not advance: what its slots hold can
        // change without a run, and the menu and the chips follow it.
        self.refresh_slots();
        let Some(session) = &mut self.session else {
            return;
        };
        if self.mute_pending && session.audio.available() {
            session.audio.toggle_mute();
            self.mute_pending = false;
        }
        if pause != self.modal_paused {
            session.occlusion_changed(pause, elapsed);
            self.modal_paused = pause;
        }
        let (advance, deadline) = machine_view::pacing::wake(
            elapsed,
            session.next_paint,
            session.running && session.occluded_since.is_none(),
        );
        session.next_paint = deadline;
        if advance {
            if let Err(error) = session.advance_with_warnings(elapsed, warnings) {
                self.host_failure(error.to_string());
                return;
            }
            let failed = session.error.as_ref().map(ToString::to_string);
            let stopped = !session.running;
            let halted = session.halted;
            self.refresh_slots();
            if let Some(error) = failed {
                self.message = format!("Machine stopped: {error}");
                self.dialog = Some(Dialog::Message(MessageBox::refusal(
                    "Machine",
                    format!("The machine stopped: {error}"),
                )));
            } else if stopped {
                self.message = if halted { HALTED } else { SWITCHED_OFF }.into();
            }
        }
        if let Some(deadline) = deadline {
            ctx.request_repaint_after(deadline.saturating_sub(elapsed));
        }
        if let Some(session) = &mut self.session {
            if session.audio.check_failure(warnings) {
                self.message = "Host audio unavailable".into();
            }
        }
        if let Some(wait) = self.next_light_off() {
            ctx.request_repaint_after(wait);
        }
    }

    /// A host failure (storage, presentation) ends the machine's run: it
    /// takes no more input, and Stop is what is left, as for every machine.
    fn host_failure(&mut self, error: String) {
        if let Some(session) = &mut self.session {
            session.release_pointer();
            session.running = false;
            session.host_failed = true;
        }
        self.paused = false;
        self.message = error.clone();
        self.dialog = Some(Dialog::Message(MessageBox::refusal("Machine error", error)));
    }

    /// Carry out `command` at host time `elapsed`. A command the machine's
    /// description does not offer does nothing. The machine's messages go
    /// to `warnings`.
    pub fn command(
        &mut self,
        command: Command,
        elapsed: Duration,
        warnings: &mut impl Write,
    ) -> Option<Request> {
        match command {
            Command::Pause => {
                // Only a running machine pauses; a stopped one waits for
                // Power cycle or Stop, and says so in the status line.
                if !self.running() {
                    return None;
                }
                if let Some(session) = &mut self.session {
                    session.release_pointer();
                    session.resync(elapsed);
                }
                self.paused = !self.paused;
                self.message = if self.paused { "Paused" } else { "Running" }.into();
            }
            Command::Reset => {
                if !self.offers.reset {
                    return None;
                }
                let Some(session) = &mut self.session else {
                    return None;
                };
                match session.reset(ResetKind::Soft, elapsed, warnings) {
                    Ok(()) => {
                        wake(session, elapsed);
                        self.message = "Reset".into();
                    }
                    Err(error) => self.message = format!("Reset: {error}"),
                }
            }
            Command::PowerCycle => self.power_cycle(elapsed, warnings),
            Command::Stop => return Some(self.stop(warnings)),
            Command::Input => {
                if self.offers.input && self.session.is_some() {
                    if let Some(session) = &mut self.session {
                        session.release_pointer();
                    }
                    self.dialog = Some(Dialog::Input(input_dialog::InputDialog::new(
                        self.bindings.clone(),
                    )));
                }
            }
            Command::Eject(slot) => self.change_medium(slot, MediaAction::Eject),
            Command::Next(slot) => self.change_medium(slot, MediaAction::Next),
            Command::Insert(slot, entry) => self.change_medium(slot, MediaAction::Insert(entry)),
            Command::ScaleToWindow => self.integer_scale = false,
            Command::IntegerScale => self.integer_scale = true,
            Command::ShowOverscan => {
                if self.offers().show_overscan {
                    self.show_overscan = !self.show_overscan;
                    self.message = if self.show_overscan {
                        "Showing the whole picture"
                    } else {
                        "Showing the visible part"
                    }
                    .into();
                }
            }
            Command::Fullscreen => return Some(Request::Fullscreen),
            Command::Mute => {
                if let Some(session) = &mut self.session {
                    session.audio.toggle_mute();
                }
            }
            Command::Speed(speed) => {
                self.speed = speed;
                if let Some(session) = &mut self.session {
                    session.select_speed(speed, elapsed);
                }
            }
            Command::FastForward(held) => {
                if let Some(session) = &mut self.session {
                    session.hold_fast_forward(held, elapsed);
                }
            }
            Command::Release => self.release_pointer(),
        }
        None
    }

    /// Queue a media change for the next run boundary, as the media keys do.
    fn change_medium(&mut self, slot: SlotId, action: MediaAction) {
        let Some(view) = self.slots.iter().find(|view| view.id == slot) else {
            return;
        };
        let Some(session) = &mut self.session else {
            return;
        };
        // The menu's checks, also for the application's hotkeys.
        let accepts = view
            .state
            .as_ref()
            .is_some_and(|state| state.accepts_changes);
        if !session.takes_media() || !accepts {
            return;
        }
        let catalogue = view.state.as_ref().map(|state| &state.catalogue[..]);
        let name = |entry: usize| catalogue.and_then(|c| c.get(entry)).map(|m| m.name.clone());
        let now = view.state.as_ref().and_then(|state| state.inserted);
        let offered = match action {
            MediaAction::Eject => view.inserted().is_some(),
            MediaAction::Next => catalogue.is_some_and(|c| !c.is_empty()),
            MediaAction::Insert(entry) => catalogue.is_some_and(|c| entry < c.len()),
            _ => false,
        };
        if !offered {
            return;
        }
        let message = match action {
            MediaAction::Eject => Some("ejected".to_owned()),
            MediaAction::Next => {
                let count = catalogue.map_or(0, <[_]>::len);
                (count > 0)
                    .then(|| now.map_or(0, |entry| (entry + 1) % count))
                    .and_then(name)
            }
            MediaAction::Insert(entry) => name(entry),
            _ => None,
        };
        session.media_events.push((slot, action));
        if let Some(message) = message {
            self.message = format!("{}: {message}", view.label);
        }
    }

    /// Power cycle: a hard reset when the machine lists one and still runs
    /// or waits halted; otherwise the machine finishes and starts again from
    /// the same file, as a fresh machine.
    fn power_cycle(&mut self, elapsed: Duration, warnings: &mut impl Write) {
        if self.offers.power_cycle_by_reset {
            if let Some(session) = self.session.as_mut().filter(|s| s.takes_media()) {
                match session.reset(ResetKind::Hard, elapsed, warnings) {
                    Ok(()) => {
                        wake(session, elapsed);
                        self.message = "Power cycled".into();
                        self.paused = false;
                    }
                    Err(error) => self.message = format!("Power cycle: {error}"),
                }
                return;
            }
        }
        if let Some(mut old) = self.session.take() {
            self.mute_pending |= old.audio.user_muted();
            old.release_pointer();
            if (old.running || old.halted) && old.error.is_none() {
                if let Err(error) = old.finish(warnings) {
                    let _ = writeln!(warnings, "{error}");
                }
            }
            // Dropped here, so its storage lets go of the machine's records
            // before the next machine opens them.
        }
        self.texture = None;
        self.layout = None;
        match new_session(&self.prepared, &self.bindings, elapsed, warnings) {
            Ok(mut session) => {
                session.window = self.window.clone();
                if self.audio_opened {
                    session.audio.open(warnings);
                }
                if self.speed != Speed::Normal {
                    session.select_speed(self.speed, elapsed);
                }
                #[cfg(test)]
                {
                    session.full_slices = self.full_slices;
                }
                let (offers, pads, slots) =
                    describe(session.machine().description(), session.display());
                (self.offers, self.pads, self.slots) = (offers, pads, slots);
                self.session = Some(session);
                self.paused = false;
                self.modal_paused = false;
                self.message = "Power cycled".into();
                self.refresh_slots();
            }
            Err(error) => {
                self.paused = false;
                self.message = STOPPED.into();
                self.dialog = Some(Dialog::Message(MessageBox::refusal(
                    "Power cycle",
                    format!("The machine could not start again: {error}"),
                )));
            }
        }
    }

    /// Finish the machine, once, and say so to the application.
    fn stop(&mut self, warnings: &mut impl Write) -> Request {
        let mut failure = None;
        if let Some(mut session) = self.session.take() {
            session.release_pointer();
            if (session.running || session.halted) && session.error.is_none() {
                if let Err(error) = session.finish(warnings) {
                    failure = Some(error.to_string());
                }
            }
            if let Some(error) = &session.error {
                failure.get_or_insert_with(|| error.to_string());
            }
        }
        self.dialog = None;
        self.texture = None;
        self.layout = None;
        Request::Stopped { failure }
    }

    /// A host key for the machine, or for the Input dialog while one of its
    /// rows waits for a key. No press reaches the machine while a dialog
    /// shows or the user paused it; releases always do, so no key stays held.
    pub fn key_event(&mut self, key: KeyCode, state: ElementState) {
        let pressed = state == ElementState::Pressed;
        if let Some(Dialog::Input(input)) = &mut self.dialog {
            if pressed {
                input.key(key);
            }
        }
        if pressed && (self.dialog.is_some() || self.paused) {
            return;
        }
        if let Some(session) = &mut self.session {
            session.key_event(key, state);
        }
    }

    /// A host key straight for the guest, past the session's host chords:
    /// the keys the application takes from the macOS system shortcuts. The
    /// same rules as [`Player::key_event`] hold back presses.
    pub fn guest_key(&mut self, key: KeyCode, state: ElementState) {
        if state == ElementState::Pressed && (self.dialog.is_some() || self.paused) {
            return;
        }
        if let Some(session) = &mut self.session {
            session.guest_key(key, state);
        }
    }

    /// Capture the pointer again, without a click, after a question of the
    /// application (a cancelled stop prompt) interrupted the capture.
    pub fn recapture_pointer(&mut self) {
        if self.dialog.is_none() && !self.paused {
            if let Some(session) = &mut self.session {
                session.recapture_pointer();
            }
        }
    }

    /// A host mouse button: it captures a relative pointer on a click over
    /// the screen and works the guest's buttons, and it is an absolute
    /// pointer's trigger wherever the pointer is.
    pub fn mouse_button(&mut self, button: MouseButton, state: ElementState) {
        if state == ElementState::Pressed && (self.dialog.is_some() || self.paused) {
            return;
        }
        if let Some(session) = &mut self.session {
            session.mouse_button(button, state);
        }
    }

    /// Raw pointer motion, for a captured relative pointer.
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        if self.dialog.is_some() || self.paused {
            return;
        }
        if let Some(session) = &mut self.session {
            session.mouse_motion(dx, dy);
        }
    }

    /// Whether the host pointer is over the screen, for capture on a click.
    pub fn pointer_inside(&mut self, inside: bool) {
        if let Some(session) = &mut self.session {
            session.mouse.pointer_inside(inside);
        }
    }

    /// The window gained or lost the keyboard: a loss releases every held
    /// key and button and tells an absolute pointer it left.
    pub fn focus_changed(&mut self, focused: bool) {
        if let Some(session) = &mut self.session {
            session.focus_changed(focused);
        }
    }

    /// Give the pointer back to the host.
    pub fn release_pointer(&mut self) {
        if let Some(session) = &mut self.session {
            session.release_pointer();
        }
    }

    /// Ask the machine which media its removable slots hold, and light the
    /// chips of the slots that were busy.
    fn refresh_slots(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        if self.slots.is_empty() {
            return;
        }
        let states = session.machine_mut().slot_states();
        for view in &mut self.slots {
            view.state = states.iter().find(|state| state.id == view.id).cloned();
            if view.state.as_ref().is_some_and(|state| state.active) {
                view.lit_until = Some(self.now.saturating_add(LIGHT_HOLD));
            }
        }
    }

    fn next_light_off(&self) -> Option<Duration> {
        self.slots
            .iter()
            .filter_map(|view| view.lit_until)
            .filter(|&until| until > self.now)
            .min()
            .map(|until| until - self.now)
    }

    #[cfg(test)]
    fn session(&mut self) -> &mut Session {
        self.session.as_mut().expect("the machine runs")
    }
}

/// Paint and advance again from host time `elapsed`: a halted machine that
/// a reset set running had no next frame to wait for.
fn wake(session: &mut Session, elapsed: Duration) {
    if session.running && session.next_paint.is_none() {
        session.next_paint = Some(elapsed);
    }
}

/// The machine for `prepared`, in a session on its first display with
/// `bindings` as its pad keys, its pacing anchored at host time `elapsed`.
fn new_session(
    prepared: &Prepared,
    bindings: &PadBindings,
    elapsed: Duration,
    warnings: &mut impl Write,
) -> Result<Session, Box<dyn Error>> {
    let machine = core_launch::start(prepared)?;
    let display = machine
        .description()
        .displays
        .first()
        .map_or(DisplayId(0), |display| display.id);
    let mut session = Session::new(machine, display, warnings)?;
    session.set_pad_bindings(bindings.clone());
    session.resync(elapsed);
    session.next_paint = Some(elapsed);
    Ok(session)
}

/// The label and button count of the absolute pointer on `display`, if any.
fn aimed_label(description: &MachineDescription, display: DisplayId) -> Option<(&str, u8)> {
    description.inputs.iter().find_map(|port| match port.kind {
        InputKind::AbsolutePointer {
            display: aimed,
            buttons,
        } if aimed == display => Some((port.label.as_str(), buttons)),
        _ => None,
    })
}

/// What the description offers, its pad ports and its removable slots.
fn describe(
    description: &MachineDescription,
    display: DisplayId,
) -> (Offers, Vec<PadPort>, Vec<SlotView>) {
    let resets = description.features.resets;
    let mut pads: Vec<(PortId, PadPort)> = description
        .inputs
        .iter()
        .filter_map(|port| match &port.kind {
            InputKind::Pad { controls } => Some((
                port.id,
                PadPort {
                    label: port.label.clone(),
                    controls: controls.clone(),
                },
            )),
            _ => None,
        })
        .collect();
    pads.sort_by_key(|(port, _)| *port);
    let slots: Vec<SlotView> = description
        .slots
        .iter()
        .filter(|slot| slot.removable)
        .map(|slot| SlotView {
            id: slot.id,
            label: slot.label.clone(),
            state: None,
            lit_until: None,
        })
        .collect();
    let offers = Offers {
        reset: resets.contains(&ResetKind::Soft),
        power_cycle_by_reset: resets.contains(&ResetKind::Hard),
        media: slots.iter().map(|slot| slot.id).collect(),
        input: !pads.is_empty(),
        show_overscan: false,
        pointer_capture: description
            .inputs
            .iter()
            .any(|port| matches!(port.kind, InputKind::RelativePointer { .. })),
        aiming: aimed_label(description, display).is_some(),
    };
    (
        offers,
        pads.into_iter().map(|(_, pad)| pad).collect(),
        slots,
    )
}

#[cfg(test)]
mod tests;
