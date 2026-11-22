// SPDX-License-Identifier: AGPL-3.0-or-later
//! One egui desktop surface around the existing paced machine session.
//!
//! Raw winit events remain intact because guest scan codes describe physical
//! positions, not egui text editing keys. See `design/spec.md` §9.4 and
//! `backlog/docs/design/machine-images.md` §7.

use crate::{
    media_dialog::{ApplyPending as _, LibraryInsertView as _},
    shell_model::{self, Command, Enablement, LibraryEntry, State},
    Args, Presenter,
};
use egui::{Color32, Sense, Stroke, Vec2};
use machine_file::Describe;
use std::{
    cell::RefCell, collections::BTreeSet, error::Error, io, rc::Rc, sync::Arc, time::Instant,
};
use ui_kit::{
    file_dialog,
    skin::{
        audio_mark, bevel, configure_context, escape_pressed, gradient_title, skin_visuals,
        window_title, EJECT_MARK,
    },
};
use unemu_core_api::{MediaAction, SlotId};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Default)]
struct Events {
    queue: Vec<HostEvent>,
    guest_focus: bool,
    captured: bool,
    /// The guest takes macOS system shortcuts now: the keyboard tap may hand
    /// it the keys it takes from macOS.
    system_keys: bool,
    window: Option<Arc<Window>>,
    screen: Option<egui::Rect>,
    pointer: egui::Pos2,
    pixels_per_point: f32,
    session: u64,
    /// Requests from the native application menu and the system's quit question.
    native: crate::app_menu::NativeRequests,
}

enum HostEvent {
    Window(Box<WindowEvent>),
    Motion(f64, f64),
    Key(HostKey),
}

// Repaints can start a new session or change focus before queued input is read.
#[derive(Clone, Copy)]
struct HostKey {
    key: KeyCode,
    shortcut_key: Option<KeyCode>,
    state: ElementState,
    repeat: bool,
    synthetic: bool,
    guest_focus: bool,
    session: u64,
}

impl Events {
    #[cfg(test)]
    fn key(&mut self, key: KeyCode, state: ElementState) -> bool {
        self.keyboard(key, state, false, false, Some(key))
    }

    fn keyboard(
        &mut self,
        key: KeyCode,
        state: ElementState,
        repeat: bool,
        synthetic: bool,
        shortcut_key: Option<KeyCode>,
    ) -> bool {
        self.queue.push(HostEvent::Key(HostKey {
            key,
            shortcut_key,
            state,
            repeat,
            synthetic,
            guest_focus: self.guest_focus,
            session: self.session,
        }));
        !self.guest_focus
    }

    fn forward(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::KeyboardInput {
            event,
            is_synthetic,
            ..
        } = event
        {
            return if let PhysicalKey::Code(key) = event.physical_key {
                self.keyboard(
                    key,
                    event.state,
                    event.repeat,
                    *is_synthetic,
                    shell_model::logical_shortcut_key(&event.logical_key),
                )
            } else {
                !self.guest_focus
            };
        }
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = egui::pos2(position.x as f32, position.y as f32)
                    / self.pixels_per_point.max(1.0)
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } if !self.captured => {
                self.guest_focus = self
                    .screen
                    .is_some_and(|screen| screen.contains(self.pointer));
            }
            WindowEvent::Focused(false) => {
                self.guest_focus = false;
                self.captured = false;
            }
            _ => {}
        }
        if matches!(
            event,
            WindowEvent::Focused(_)
                | WindowEvent::ModifiersChanged(_)
                | WindowEvent::MouseInput { .. }
                | WindowEvent::CursorMoved { .. }
                | WindowEvent::CursorLeft { .. }
                | WindowEvent::Occluded(_)
        ) {
            self.window(event.clone());
        }
        match event {
            WindowEvent::Ime(_) => !self.guest_focus,
            WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::CursorEntered { .. }
            | WindowEvent::CursorLeft { .. }
            | WindowEvent::Touch(_)
            | WindowEvent::PinchGesture { .. }
            | WindowEvent::PanGesture { .. }
            | WindowEvent::DoubleTapGesture { .. }
            | WindowEvent::RotationGesture { .. }
            | WindowEvent::TouchpadPressure { .. } => !self.captured,
            _ => true,
        }
    }

    fn repaint(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window(&mut self, event: WindowEvent) {
        self.queue.push(HostEvent::Window(Box::new(event)));
    }
    fn motion(&mut self, dx: f64, dy: f64) {
        if let Some(HostEvent::Motion(x, y)) = self.queue.last_mut() {
            *x += dx;
            *y += dy;
        } else {
            self.queue.push(HostEvent::Motion(dx, dy));
        }
    }
}

struct Runner<'a> {
    eframe: eframe::EframeWinitApplication<'a>,
    events: Rc<RefCell<Events>>,
}

impl ApplicationHandler<eframe::UserEvent> for Runner<'_> {
    fn resumed(&mut self, events: &ActiveEventLoop) {
        self.events
            .borrow_mut()
            .window(WindowEvent::Occluded(false));
        self.eframe.resumed(events);
    }
    fn new_events(&mut self, events: &ActiveEventLoop, cause: StartCause) {
        self.eframe.new_events(events, cause);
    }
    fn user_event(&mut self, events: &ActiveEventLoop, event: eframe::UserEvent) {
        self.eframe.user_event(events, event);
    }
    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let forward = self.events.borrow_mut().forward(&event);
        if forward {
            self.eframe.window_event(events, id, event);
        } else {
            self.events.borrow().repaint();
        }
    }
    fn device_event(
        &mut self,
        events: &ActiveEventLoop,
        id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = &event {
            self.events.borrow_mut().motion(delta.0, delta.1);
        }
        if self.events.borrow().captured {
            self.events.borrow().repaint();
        } else {
            self.eframe.device_event(events, id, event);
        }
    }
    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        self.eframe.about_to_wait(events);
    }
    fn suspended(&mut self, events: &ActiveEventLoop) {
        self.events.borrow_mut().window(WindowEvent::Focused(false));
        self.events.borrow_mut().window(WindowEvent::Occluded(true));
        self.eframe.suspended(events);
    }
    fn exiting(&mut self, events: &ActiveEventLoop) {
        self.eframe.exiting(events);
    }
}

pub(super) fn run(
    args: Args,
    autostart: bool,
    cores: core_launch::Cores,
) -> Result<(), Box<dyn Error>> {
    let events = Rc::new(RefCell::new(Events::default()));
    let shell_events = events.clone();
    let errors = Rc::new(RefCell::new(None::<String>));
    let shell_errors = errors.clone();
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = EventLoop::<eframe::UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        // winit's default menu quits through NSApplication `terminate:` before
        // the shell can ask to stop a running machine; the shell's own
        // application menu runs Exit instead.
        builder.with_default_menu(false);
    }
    let event_loop = builder.build()?;
    // Menu items and the system's quit question reach the shell's next pass.
    // Command+H, Command+Option+H and Command+Q are the menu's key
    // equivalents, so AppKit takes those key presses before the window sees
    // them and each acts once.
    #[cfg(target_os = "macos")]
    {
        let native = events.clone();
        crate::macos_menu::install(move |request| {
            let Ok(mut events) = native.try_borrow_mut() else {
                return false;
            };
            events.native.push(request);
            events.repaint();
            true
        });
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("unemu")
            .with_icon(ui_kit::logo::icon()?)
            .with_inner_size([1040.0, 700.0])
            .with_min_inner_size([760.0, 500.0]),
        renderer: eframe::Renderer::Glow,
        // Only a focused text field takes cut, copy and paste, and only then
        // is the host clipboard read; see `text_clipboard`.
        clipboard_shortcuts: false,
        persist_window: false,
        ..Default::default()
    };
    let app = eframe::create_native(
        "unemu",
        options,
        Box::new(move |cc| {
            configure_context(&cc.egui_ctx);
            let mut shell = Shell::new(
                args,
                autostart,
                shell_events,
                shell_errors,
                cc.winit_window().cloned(),
                machine_file::firmware_discovery::default_firmware_roots(),
                cores,
            );
            shell.logo = Some(ui_kit::logo::Logo::load(&cc.egui_ctx)?);
            if autostart {
                shell.start();
            }
            shell.open_on_start(&cc.egui_ctx);
            Ok(Box::new(shell))
        }),
        &event_loop,
    );
    let mut runner = Runner {
        eframe: app,
        events,
    };
    event_loop.run_app(&mut runner)?;
    if let Some(error) = errors.borrow_mut().take() {
        return Err(error.into());
    }
    Ok(())
}

/// The CPU stop report of a failed session: the text the Machine error
/// dialog shows in monospace, and where it was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorReport {
    text: String,
    /// "cpu stop report saved to …", or why it was not.
    saved: String,
}

/// The name of the CPU stop report in the machine's workspace; each failed
/// session replaces the last one.
const CPU_STOP_REPORT_FILE: &str = "last-cpu-stop.txt";

/// The workspace folder that holds the report. Discarding or clearing an
/// image's workspace removes every folder in it, so the report goes with the
/// session it describes.
const CPU_STOP_REPORT_DIR: &str = "diagnostics";

/// OK and Copy for a message dialog. Copy puts `copy` on the host clipboard
/// through egui's copy output; returns whether OK was pressed.
fn message_buttons(ui: &mut egui::Ui, copy: &str) -> bool {
    ui.horizontal(|ui| {
        let ok = ui.button("OK").clicked();
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(copy.to_owned());
        }
        ok
    })
    .inner
}

struct Shell {
    args: Args,
    library: Vec<LibraryEntry>,
    image_library: crate::library_view::LibraryView,
    /// Tools › Diagnostics › Record exceptions; off at every launch.
    record_exceptions: bool,
    retain_workspace_on_stop: bool,
    exit_requested: bool,
    store_path_input: String,
    selected: Option<usize>,
    presenter: Option<Presenter>,
    paused: bool,
    classic: bool,
    toolbar: bool,
    statusbar: bool,
    fullscreen: bool,
    integer_scale: bool,
    dialog: Option<(&'static str, String)>,
    /// The CPU stop report the Machine error dialog shows under its message.
    error_report: Option<ErrorReport>,
    message: String,
    texture: Option<egui::TextureHandle>,
    logo: Option<ui_kit::logo::Logo>,
    events: Rc<RefCell<Events>>,
    errors: Rc<RefCell<Option<String>>>,
    window: Option<Arc<Window>>,
    screen: egui::Rect,
    guest_focus: bool,
    pointer: egui::Pos2,
    modifiers: [bool; 8],
    macos_shortcuts: bool,
    host_modifiers: ModifiersState,
    fast_forward_key: Option<KeyCode>,
    consumed: BTreeSet<KeyCode>,
    held: BTreeSet<KeyCode>,
    guest_held: BTreeSet<KeyCode>,
    session: u64,
    modal_paused: bool,
    hidden: bool,
    minimized: bool,
    open_path: String,
    preferences: crate::preferences::Preferences,
    preferences_draft: crate::preferences::Preferences,
    preferences_path: Option<std::path::PathBuf>,
    /// Why `preferences_path` is unset: no platform config directory, or the
    /// copy from the project's former directory failed.
    preferences_path_error: Option<String>,
    rom_folder_input: String,
    library_revision: u64,
    readiness: Option<ReadinessJob>,
    prepared: std::collections::BTreeMap<usize, machine_file::PreparedMachineFile>,
    discovery_session: machine_file::firmware_discovery::DiscoverySession,
    discovery_roots: Vec<machine_file::firmware_discovery::FirmwareRoot>,
    start_request: Option<usize>,
    checked_selection: Option<usize>,
    insert_dialog: Option<crate::media_dialog::InsertDialog>,
    /// The media picker: Insert… of a drive.
    picker: Option<crate::media_picker::Picker>,
    /// Floppies inserted while the selected machine is stopped; Start puts
    /// them into its drives before POST.
    pending_media: Option<crate::media_dialog::PendingMedia>,
    blank_disk: Option<crate::media_dialog::BlankDisk>,
    settings: Option<crate::settings_model::SettingsModel>,
    preferences_dialog: Option<crate::preferences_sheet::PreferencesDialog>,
    /// The operating system's registry credential store, shared by every
    /// Preferences dialog of this session so session-only logins persist.
    credential_store: Arc<dyn image_registry::credential_store::CredentialStore>,
    discovery_warnings: BTreeSet<String>,
    discovery_control: machine_file::firmware_discovery::DiscoveryControl,
    firmware_cache: Option<std::path::PathBuf>,
    default_store_root: Option<std::path::PathBuf>,
    default_firmware_roots: Vec<machine_file::firmware_discovery::FirmwareRoot>,
    rescan: bool,
    refresh_listing: bool,
    discovery_finished: bool,
    stop_prompt: Option<StopPrompt>,
    /// Guest keyboard focus and pointer capture when the stop prompt opened.
    stop_prompt_input: (bool, bool),
    /// Input to hand back to the guest once Cancel has closed the prompt.
    restore_input: Option<(bool, bool)>,
    /// Quit waits for an answer while a push or pull runs: "push" or "pull".
    transfer_prompt: Option<&'static str>,
    /// The opt-in keyboard tap that hands macOS system shortcuts to the guest.
    system_keys: crate::system_keys::SystemKeys,
    /// The host clipboard a paste into a text field reads.
    clipboard: Box<dyn ui_kit::text_clipboard::HostClipboard>,
    /// The host file dialog, which answers on a later frame. It is no shell
    /// dialog: a running machine keeps running while it is open.
    file_dialogs: file_dialog::FileDialogs<FileFor>,
    /// Create and insert while the library worker builds the disks: whose
    /// drives they go into once made.
    creating: Option<DriveOwner>,
    /// The compiled-in cores, which File › Open… and `--open` ask.
    cores: Arc<core_launch::Cores>,
    /// Where machines started from a file keep what they leave behind, or
    /// why this host has no such place.
    host_dirs: Result<core_launch::HostDirs, String>,
    /// The machine started from a file. The shell runs one machine at a
    /// time: this one or `presenter`.
    player: Option<player_ui::Player>,
    /// What the player's menus asked for in this pass, carried out once
    /// the menus have drawn.
    player_commands: Vec<player_ui::Command>,
    /// The file File › Open… or `--open` prepares on a worker thread:
    /// hashing a large file must not hold up the window.
    /// The status line's message before "Opening FILE…" took its place.
    opening: Option<(std::sync::mpsc::Receiver<OpenResult>, String)>,
    /// A prepared file waiting for the stop prompt's answer.
    opened: Option<core_launch::Prepared>,
    /// Why a file did not open, or its machine did not start.
    open_refusal: Option<player_ui::MessageBox>,
    /// What the player's machine said while it ran, which a test reads
    /// where the program writes it to standard error.
    #[cfg(test)]
    player_log: Vec<u8>,
}

/// What preparing a file gave: a machine to start, or why not.
type OpenResult = Result<core_launch::Outcome, core_launch::LaunchError>;

/// Who asked for the host file dialog; the answer goes back there on a later
/// frame. Each variant says when its answer is dropped instead.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FileFor {
    /// Browse… of the Insert floppy dialog of a drive. Dropped unless that
    /// dialog is still open for the same drive.
    InsertImage(u8),
    /// Save as… for the separate journal in the Insert floppy dialog of a
    /// drive. Dropped unless that dialog is still open for the same drive.
    InsertJournal(u8),
    /// Browse… of Create blank disk. Dropped once that dialog has closed.
    BlankDisk,
    /// Browse for an image file… of a drive's media picker, for the session
    /// or stopped machine it was asked in. Dropped, with a status message,
    /// when another one is current or another shell dialog is open.
    DriveImage(u8, DriveOwner),
    /// File › Import ROM folder…. Dropped, with a status message, when the
    /// command is no longer allowed.
    RomImport,
    /// The source archive of a firmware package, with the package Edit
    /// metadata… was open for when asked. For Edit, dropped with a status
    /// message unless that dialog is still open for the same package; for an
    /// import, the import dialog ignores it once closed.
    FirmwareSource(crate::library_view::firmware::SourceTarget, Option<String>),
    /// Import installation media…, which ignores it once closed.
    InstallMedia(crate::library_view::install_media::Pick),
    /// Browse… of the ROM folders dialog. Dropped once that dialog has closed.
    RomFolder,
    /// Add files… of the media picker's Create view. Dropped unless the
    /// picker still shows that view.
    CreateFiles,
    /// Add folder… of the media picker's Create view, as Add files….
    CreateFolder,
    /// File › Open…: the file is prepared for a machine whatever the shell
    /// shows meanwhile.
    Open,
}

/// Whose drives a picked image goes into.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DriveOwner {
    /// The running machine, by session number; each start and stop counts.
    Session(u64),
    /// The stopped machine whose drives hold floppies for its next Start.
    Stopped(crate::media_dialog::PendingOwner),
}

/// A shell action that would end a running session, held while the user
/// decides whether to stop the machine for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopPrompt {
    Library,
    OpenMachine,
    /// Start the machine of the file Open… prepared, held in `opened`.
    OpenFile,
    Exit,
}

struct ReadinessJob {
    index: usize,
    revision: u64,
    user_cancelled: bool,
    control: machine_file::firmware_discovery::DiscoveryControl,
    receiver: std::sync::mpsc::Receiver<shell_model::PreparedEntry>,
}

impl Shell {
    fn poll_images(&mut self, ctx: &egui::Context) {
        self.image_library.poll(ctx);
        let reads = std::mem::take(&mut self.image_library.host_reads);
        if let Some(picker) = &mut self.picker {
            for (path, result) in reads {
                picker.create.read_done(&path, result);
            }
        }
        if let Some(result) = self.image_library.disks_created.take() {
            self.disks_created(result);
        }
        if let Some(message) = self.image_library.status.take() {
            self.message = message;
        }
        if let Some(error) = self.image_library.error.take() {
            let firmware = std::mem::take(&mut self.image_library.firmware_error);
            self.show_error(error.clone());
            if firmware {
                self.dialog = Some(("Missing firmware", error));
            }
        }
        if let Some(transfer) = self.image_library.launched.take() {
            let inserts = std::mem::take(&mut self.image_library.launched_inserts);
            let kept = std::mem::take(&mut self.image_library.start_catalogue);
            if self.player.is_some() {
                // A Start the library worker had under way when a file's
                // machine began: its workspace is closed and kept as it was,
                // and nothing starts beside the running machine.
                drop(transfer);
                self.image_library.start_notes.clear();
                self.image_library.finish_with_failure(true, false);
                self.message =
                    "The machine did not start: a machine opened from a file runs".into();
                return;
            }
            self.selected = None;
            let catalogue_paths = transfer.catalogue_paths;
            let prepared = transfer.launch.prepared.with_options(vec![
                (
                    "allow-unlisted-firmware".into(),
                    self.args.options.allow_unlisted_firmware.to_string(),
                ),
                ("speaker-trace".into(), self.args.speaker_trace.to_string()),
            ]);
            let result = core_launch::start_with(&prepared, |request| {
                unemu_pc_core::PcCore.create_machine(request)
            })
            .map_err(|e| e.to_string())
            .and_then(|machine| {
                let display = display_device(&machine.config().devices)
                    .ok_or_else(|| "Machine has no display card".to_string())?;
                Presenter::from_machine(machine, display, &mut io::stderr())
                    .map(|presenter| presenter.with_catalogue_paths(catalogue_paths))
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(mut presenter) => {
                    presenter.audio.open(&mut io::stderr());
                    self.started(presenter);
                    self.name_inserted_disks(&inserts);
                    self.restore_catalogue(kept);
                    // What the Start left out, once the machine runs.
                    let notes = std::mem::take(&mut self.image_library.start_notes);
                    if !notes.is_empty() {
                        self.message = notes.join(" ");
                    }
                    if self.image_library.active_machine.is_some() {
                        // The machine's record takes this run only now that
                        // it assembled: its pending disks are in its drives.
                        self.image_library
                            .send(crate::library_worker::Request::Assembled);
                    }
                    if let Some((row, _, _)) = &self.image_library.active {
                        let owner =
                            crate::media_dialog::PendingOwner::Image(row.descriptor.digest.clone());
                        self.insert_pending_media(&owner);
                    }
                }
                Err(e) => {
                    self.image_library.start_notes.clear();
                    self.image_library.finish_with_failure(false, true);
                    self.show_error(e);
                }
            }
        }
    }

    fn open_settings(&mut self) {
        self.release();
        if let Some(index) = self.selected {
            match crate::settings_model::SettingsModel::open(
                &self.library[index].path,
                self.presenter.is_some(),
                crate::machines_dir(),
            ) {
                Ok(model) => self.settings = Some(model),
                Err(error) => self.dialog = Some(("Settings", error)),
            }
        }
    }

    fn settings_modal(&mut self, ctx: &egui::Context) {
        let Some(model) = &mut self.settings else {
            return;
        };
        model.read_only = self.presenter.is_some();
        match crate::settings_sheet::show(ctx, model, self.classic) {
            crate::settings_sheet::Action::None => {}
            crate::settings_sheet::Action::Cancel => self.settings = None,
            crate::settings_sheet::Action::Save { close } => self.save_settings(close),
        }
    }

    fn save_settings(&mut self, close: bool) {
        let Some(model) = &mut self.settings else {
            return;
        };
        model.read_only = self.presenter.is_some();
        match model.save() {
            Ok(path) => {
                let index = if let Some(index) = self.library.iter().position(|entry| {
                    entry.path == path
                        || entry
                            .path
                            .canonicalize()
                            .ok()
                            .zip(path.canonicalize().ok())
                            .is_some_and(|(left, right)| left == right)
                }) {
                    index
                } else {
                    self.library.push(LibraryEntry::pending(path));
                    self.library.len() - 1
                };
                self.selected = Some(index);
                self.refresh_library();
                self.message = "Local machine settings saved".into();
                if close {
                    self.settings = None;
                }
            }
            Err(error) => model.error = Some(error),
        }
    }

    fn has_dialog(&self) -> bool {
        self.dialog.is_some()
            || self.stop_prompt.is_some()
            || self.transfer_prompt.is_some()
            || self.insert_dialog.is_some()
            || self.picker.is_some()
            || self.blank_disk.is_some()
            || self.settings.is_some()
            || self.preferences_dialog.is_some()
            || self.image_library.has_dialog()
            || self.open_refusal.is_some()
            || self
                .player
                .as_ref()
                .is_some_and(player_ui::Player::has_dialog)
    }

    fn confirm_insert(&mut self) {
        let Some(draft) = &mut self.insert_dialog else {
            return;
        };
        let result = if let Some(library) = &mut draft.library {
            let Some(result) = library.request_entries(&draft.slot, draft.unit) else {
                return;
            };
            result
        } else {
            draft.selection().map(|entry| (vec![entry], 0))
        };
        let (entries, chosen) = match result {
            Ok(entries) => entries,
            Err(error) => {
                if let Some(draft) = &mut self.insert_dialog {
                    draft.error = Some(error);
                }
                return;
            }
        };
        if let Some(presenter) = &mut self.presenter {
            let (assembled, config, slots) = presenter.media_parts();
            let result =
                crate::media_dialog::append_entries(assembled, config, slots, draft.unit, &entries);
            match result {
                Ok(indices) => presenter
                    .media_events
                    .push((SlotId(draft.unit), MediaAction::Insert(indices[chosen]))),
                Err(error) => {
                    if let Some(draft) = &mut self.insert_dialog {
                        draft.error = Some(error);
                    }
                    return;
                }
            }
        } else if let Some(pending) = &mut self.pending_media {
            let name = entries
                .get(chosen)
                .map(|entry| entry.medium.name.clone())
                .unwrap_or_default();
            let drive = if draft.unit == 0 { "A" } else { "B" };
            pending.insert(draft.unit, entries, chosen);
            self.message = format!("{drive}: {name} is in the drive when Start boots");
        }
        self.insert_dialog = None;
    }

    /// The catalogue Insert offers for `unit`: the session's, or while no
    /// session runs, the selected machine's.
    fn floppy_slot(&self, unit: u8) -> Option<crate::media_dialog::FloppySlot> {
        match &self.presenter {
            Some(presenter) => presenter.media_slots.get(usize::from(unit))?.clone(),
            None => self
                .pending_media
                .as_ref()?
                .slots
                .get(usize::from(unit))?
                .clone(),
        }
    }

    /// Whose drives `floppy_slot` reads now: the session, or the stopped
    /// machine holding floppies for Start.
    fn drive_owner(&self) -> Option<DriveOwner> {
        match &self.presenter {
            Some(_) => Some(DriveOwner::Session(self.session)),
            None => Some(DriveOwner::Stopped(
                self.pending_media.as_ref()?.owner.clone(),
            )),
        }
    }

    /// Floppies inserted before Start belong to the selected machine: another
    /// selection drops them, and Start takes them into the session.
    fn sync_pending_media(&mut self) {
        use crate::media_dialog::{PendingMedia, PendingOwner};
        if self.presenter.is_some() {
            self.pending_media = None;
            return;
        }
        let owner = match &self.image_library.selected {
            // A machine keeps its own pending inserts in the store. Floppies
            // inserted into an image stay with it while Start makes the
            // image's machine, which takes them before its first instruction.
            Some(row) if self.image_library.machine.is_some() => self
                .pending_media
                .as_ref()
                .filter(|pending| {
                    pending.owner == PendingOwner::Image(row.descriptor.digest.clone())
                })
                .filter(|_| {
                    self.image_library.started_from_image.is_some()
                        && self.image_library.started_from_image == self.image_library.machine
                })
                .map(|pending| pending.owner.clone()),
            Some(row) => Some(PendingOwner::Image(row.descriptor.digest.clone())),
            None => self
                .selected
                .and_then(|index| self.library.get(index))
                .map(|entry| PendingOwner::MachineFile(entry.path.clone())),
        };
        if self.pending_media.as_ref().map(|pending| &pending.owner) == owner.as_ref() {
            return;
        }
        // A draft lists the previous machine's catalogue.
        self.insert_dialog = None;
        self.pending_media = owner.and_then(|owner| {
            let config = match &owner {
                PendingOwner::MachineFile(_) => {
                    let (document, origins) = self.prepared.get(&self.selected?)?.resolved();
                    document.describe(origins).ok()?.config
                }
                PendingOwner::Image(digest) => self.image_library.selected_machine(digest)?.clone(),
            };
            Some(PendingMedia::new(owner, config))
        });
    }

    /// Start puts the floppies inserted before it into their drives, before
    /// the machine's first instruction.
    fn insert_pending_media(&mut self, owner: &crate::media_dialog::PendingOwner) {
        let Some(pending) = self.pending_media.take() else {
            return;
        };
        if &pending.owner != owner {
            return;
        }
        let directory = self.library_media_dir().ok();
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        let failures = pending.apply(presenter, directory.as_deref());
        if !failures.is_empty() {
            self.dialog = Some((
                "Insert floppy",
                format!(
                    "Start left out floppies inserted before it:\n{}",
                    failures.join("\n")
                ),
            ));
        }
    }

    /// Library disks journal into the running session's workspace: the image
    /// workspace for an image session, the machine file's local media folder
    /// otherwise. Never beside another machine file, never in the store's blobs.
    fn library_media_dir(&self) -> Result<std::path::PathBuf, String> {
        if let Some((_, name, _)) = &self.image_library.active {
            let root = self
                .image_library
                .root
                .as_ref()
                .ok_or("The image store folder is not set")?;
            let store = image_store::Store::open_existing(root).map_err(|e| e.to_string())?;
            return store
                .workspace_path(name)
                .map(|workspace| workspace.join("media"))
                .map_err(|e| e.to_string());
        }
        if self.presenter.is_none() {
            use crate::media_dialog::PendingOwner;
            match self.pending_media.as_ref().map(|pending| &pending.owner) {
                Some(PendingOwner::MachineFile(path)) => {
                    return image_store::location::machine_media_dir(path)
                        .map_err(|e| e.to_string());
                }
                // An image's workspace opens only at Start, which moves the
                // journals there; before it, library disks go in
                // write-protected, so nothing is written under this folder.
                Some(PendingOwner::Image(_)) => {
                    return self
                        .image_library
                        .root
                        .clone()
                        .ok_or_else(|| "The image store folder is not set".into());
                }
                None => {}
            }
        }
        image_store::location::machine_media_dir(&self.args.machine).map_err(|e| e.to_string())
    }

    fn open_insert_library(&mut self) {
        let store_root = self.preferences.store_root.clone();
        self.open_insert_library_at(store_root, None);
    }

    /// Insert of the library window's selected set: the drive's From library…
    /// view on the library's own store, with that set chosen.
    fn open_insert_library_at(
        &mut self,
        store_root: Option<std::path::PathBuf>,
        set: Option<image_store::Digest>,
    ) {
        let workspace = self.library_media_dir();
        let before_image_start = self.presenter.is_none()
            && matches!(
                self.pending_media.as_ref().map(|pending| &pending.owner),
                Some(crate::media_dialog::PendingOwner::Image(_))
            );
        if let Some(draft) = &mut self.insert_dialog {
            match workspace {
                Ok(workspace) => {
                    draft.open_library_at(&workspace, store_root.as_deref(), set);
                    if let Some(library) = draft.library.as_mut().filter(|_| before_image_start) {
                        library.write_protected_only("Before Start, library disks go in write-protected: their journals belong to the session workspace, which Start opens.");
                    }
                }
                Err(error) => draft.error = Some(error),
            }
        }
    }

    /// A set the media picker chose goes in without the old dialog, once the
    /// set is verified. Meanwhile a small dialog shows the progress and can
    /// cancel. A refusal only says why: the machine keeps running, as a
    /// refused disk never stops a real PC.
    fn automatic_insert(&mut self, ctx: &egui::Context) {
        if !self.insert_dialog.as_ref().is_some_and(|d| d.automatic) {
            return;
        }
        if let Some(error) = self.insert_dialog.as_mut().and_then(|d| d.error.take()) {
            self.insert_dialog = None;
            self.message = "The disk did not go in".into();
            self.dialog = Some(("Insert disk", error));
            return;
        }
        self.confirm_insert();
        if self.insert_dialog.is_none() {
            if self.message == "Verifying installation media…" {
                self.message = "Inserted".into();
            }
            return;
        }
        let progress = self
            .insert_dialog
            .as_ref()
            .and_then(|d| d.library.as_ref())
            .map(|library| library.progress.clone())
            .filter(|progress| !progress.is_empty())
            .unwrap_or_else(|| "Verifying installation media…".into());
        let mut cancel = false;
        egui::Modal::new(egui::Id::unique("insert-progress")).show(ctx, |ui| {
            ui.set_width(360.0);
            if self.classic {
                gradient_title(ui, "Insert disk");
            } else {
                ui.heading("Insert disk");
            }
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(&progress);
            });
            cancel = ui.button("Cancel").clicked() || ui.input_mut(escape_pressed);
        });
        if cancel {
            // The verification thread's result is dropped with the draft.
            self.insert_dialog = None;
            self.message = "Insert cancelled".into();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }

    fn media_modals(&mut self, ctx: &egui::Context) {
        let mut insert = false;
        let mut cancel = false;
        let mut open_library = false;
        let mut asked = None;
        let journal_place = match &self.image_library.active {
            Some((_, _, kind)) if kind.disposable() => {
                "Journal in this Gaming-mode workspace; Discard removes it"
            }
            Some(_) => "Journal in this image's workspace",
            None => "Journal in this machine file's local media folder",
        };
        self.automatic_insert(ctx);
        if let Some(draft) = self.insert_dialog.as_mut().filter(|d| !d.automatic) {
            let title = format!(
                "{}{}:",
                if draft.library.is_some() {
                    "From library — Insert into drive "
                } else {
                    "Insert floppy into "
                },
                if draft.unit == 0 { "A" } else { "B" }
            );
            egui::Modal::new(egui::Id::unique("insert-floppy")).show(ctx, |ui| {
                ui.set_width(560.0);
                if self.classic { gradient_title(ui, &title); } else { ui.heading(title); }
                if draft.library.is_some() {
                    open_library |= draft.library_ui(ui);
                }
                insert |= draft.library.as_ref().is_some_and(|library| library.ready.is_some());
                if draft.library.is_none() {
                if !draft.browsed_only {
                ui.label("Catalogue for this machine:");
                let mut selected = None;
                egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                    for (index, entry) in draft.slot.entries.iter().enumerate() {
                        let label = format!("{} — {}", entry.medium.name, crate::media_dialog::write_name(&entry.medium.write));
                        let response = ui.add_enabled(entry.reason.is_none(), egui::Button::selectable(draft.selected == Some(index), label));
                        if response.clicked() {
                            selected = Some(index);
                        }
                        if let Some(reason) = &entry.reason { ui.add(egui::Label::new(reason).wrap()); }
                    }
                });
                if let Some(index) = selected { draft.select(Some(index)); }
                if ui.button("Browse…").clicked() {
                    asked = Some(FileFor::InsertImage(draft.unit));
                }
                } else if let Some(entry) = draft.selected.and_then(|index| draft.slot.entries.get(index)) {
                    ui.strong(&entry.medium.name);
                }
                if let Some(entry) = draft.selected.and_then(|index| draft.slot.entries.get(index)) {
                    if let Some(geometry) = entry.geometry {
                        ui.label(format!("{} · {}/{}/{} · detected from size", crate::media_dialog::geometry_name(geometry), geometry.cylinders, geometry.heads, geometry.sectors_per_track));
                    }
                    ui.label("Write mode");
                    if entry.declared {
                        ui.label(format!("{} — fixed by the machine file", crate::media_dialog::write_name(&entry.medium.write)));
                        if let assembly::WriteMode::Journal { keep_generations, .. } = entry.medium.write {
                            ui.label(format!("Retains {keep_generations} closed generations; base stays read-only."));
                        }
                    } else {
                        ui.radio_value(&mut draft.journal, false, "Read-only");
                        ui.radio_value(&mut draft.journal, true, "Journal (keep changes separately)");
                        if draft.journal && entry.workspace_journal.is_some() {
                            if let Some(path) = &entry.workspace_journal {
                                ui.add(egui::Label::new(format!("{journal_place}: {}", path.display())).wrap());
                            }
                            ui.label(format!("Retains {} closed generations. The library disk stays read-only.", draft.slot.journal_retention));
                        } else if draft.journal {
                            ui.label("Separate journal path");
                            ui.text_edit_singleline(&mut draft.journal_path);
                            if let assembly::ImageSource::File(base) = &entry.medium.image {
                                let directory = base.parent().unwrap_or(std::path::Path::new("."));
                                ui.label(format!("Relative paths use {}", directory.display()));
                                if ui.button("Save as…").clicked() {
                                    asked = Some(FileFor::InsertJournal(draft.unit));
                                }
                            }
                            ui.label(format!("Retains {} closed generations. The base remains immutable.", draft.slot.journal_retention));
                        }
                    }
                }
                }
                if let Some(error) = &draft.error { ui.add(egui::Label::new(error).wrap()); }
                ui.separator();
                ui.horizontal(|ui| {
                    insert |= ui.add_enabled(draft.library.as_ref().map_or_else(|| draft.selection().is_ok(), |library| library.selected.is_some() && !library.is_busy()), egui::Button::new("Insert")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                cancel |= ui.input_mut(escape_pressed);
            });
        }
        if cancel {
            self.insert_dialog = None;
        } else if insert {
            self.confirm_insert();
        } else if open_library {
            self.open_insert_library();
        }

        let mut close = false;
        let mut created = false;
        if let Some(form) = &mut self.blank_disk {
            egui::Modal::new(egui::Id::unique("blank-disk")).show(ctx, |ui| {
                ui.set_width(520.0);
                if self.classic { gradient_title(ui, "Create blank disk"); } else { ui.heading("Create blank disk"); }
                egui::ComboBox::from_label("Disk type").selected_text(crate::media_dialog::BlankDisk::FORMATS[form.format]).show_ui(ui, |ui| {
                    for (index, name) in crate::media_dialog::BlankDisk::FORMATS.iter().enumerate() { ui.selectable_value(&mut form.format, index, *name); }
                });
                if form.format == 0 {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut form.by_size, false, "CHS");
                        ui.radio_value(&mut form.by_size, true, "Size (MiB/GiB)");
                    });
                    if form.by_size { ui.text_edit_singleline(&mut form.size); } else { ui.label("Cylinders / heads / sectors"); ui.text_edit_singleline(&mut form.chs); }
                    ui.label("Unpartitioned disk. Use CHS for an exact machine-file geometry; size rounds up to 16 heads / 63 sectors. Partition and format in DOS.");
                } else {
                    ui.label("Formatted FAT12 data floppy; not a boot disk.");
                }
                ui.label("New image path (existing files are never overwritten)");
                ui.text_edit_singleline(&mut form.output);
                ui.label(format!("Relative paths use {}", form.directory.display()));
                if ui.button("Browse…").clicked() {
                    asked = Some(FileFor::BlankDisk);
                }
                if let Some(error) = &form.error { ui.add(egui::Label::new(error).wrap()); }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        match form.create() {
                            Ok(_) => { close = true; created = true; },
                            Err(error) => form.error = Some(error),
                        }
                    }
                    close |= ui.button("Cancel").clicked();
                });
                close |= ui.input_mut(escape_pressed);
            });
        }
        if close {
            self.blank_disk = None;
            if created {
                self.message = "Blank disk created; select its path in the machine file or insert it with Media › Insert…".into();
                if self.presenter.is_none() {
                    self.refresh_library();
                }
            }
        }
        if let Some(asker) = asked {
            self.ask_file(asker);
        }
    }

    /// Open the host file dialog for `asker` and return at once; the answer
    /// reaches [`Self::file_chosen`] on a later frame. While one file dialog
    /// is open another is refused, and the status bar says which one.
    fn ask_file(&mut self, asker: FileFor) {
        let Some(request) = self.file_request(&asker) else {
            return;
        };
        if self
            .file_dialogs
            .open(asker, &request, self.window.as_deref())
            .is_err()
        {
            self.message = format!(
                "{} did not open: another file dialog was open",
                request.title
            );
        }
    }

    /// The title, filter, start folder and file name each asker's dialog
    /// opens with, from the dialog state it was asked in.
    fn file_request(&self, asker: &FileFor) -> Option<file_dialog::Request> {
        use crate::library_view::{firmware::SourceTarget, install_media::Pick};
        use file_dialog::{Kind, Request};
        Some(match asker {
            FileFor::InsertImage(_) | FileFor::DriveImage(..) => {
                Request::new(Kind::File, "Choose a floppy image")
                    .filter("Disk images", &["img", "ima", "dsk"])
            }
            FileFor::InsertJournal(unit) => {
                let draft = self.insert_dialog.as_ref().filter(|d| d.unit == *unit)?;
                let entry = draft
                    .selected
                    .and_then(|index| draft.slot.entries.get(index))?;
                let assembly::ImageSource::File(base) = &entry.medium.image else {
                    return None;
                };
                // Relative journal paths resolve against the base image's folder.
                let directory = base.parent().unwrap_or(std::path::Path::new("."));
                let path = crate::media_dialog::resolve_input(directory, &draft.journal_path);
                Request::new(Kind::Save, "Choose a separate journal")
                    .directory(if draft.journal_path.trim().is_empty() {
                        directory
                    } else {
                        path.parent().unwrap_or(directory)
                    })
                    .file_name(crate::media_dialog::filename(&path))
            }
            FileFor::BlankDisk => {
                let form = self.blank_disk.as_ref()?;
                let path = crate::media_dialog::resolve_input(&form.directory, &form.output);
                let empty = form.output.trim().is_empty();
                Request::new(Kind::Save, "Create blank disk")
                    .directory(if empty {
                        form.directory.as_path()
                    } else {
                        path.parent().unwrap_or(&form.directory)
                    })
                    .file_name(if empty {
                        "disk.img".into()
                    } else {
                        crate::media_dialog::filename(&path)
                    })
            }
            FileFor::RomImport => Request::new(Kind::Folder, "Import ROM folder"),
            FileFor::FirmwareSource(target, _) => {
                let request = Request::new(Kind::File, "Choose the source archive (zip or tar)");
                match target {
                    SourceTarget::Import(_, folder) => request.directory(folder),
                    SourceTarget::Edit => request,
                }
            }
            FileFor::InstallMedia(Pick::Files) => {
                Request::new(Kind::Files, "Choose installation images")
            }
            FileFor::InstallMedia(Pick::Folder) => {
                Request::new(Kind::Folder, "Choose a folder of installation images")
            }
            FileFor::InstallMedia(Pick::Extras) => {
                Request::new(Kind::Files, "Choose extras to store with the set")
            }
            FileFor::RomFolder => Request::new(Kind::Folder, "Locate firmware folder"),
            FileFor::CreateFiles => Request::new(Kind::Files, "Add files to the disk"),
            FileFor::CreateFolder => Request::new(Kind::Folder, "Add a folder to the disk"),
            FileFor::Open => {
                // The host's dialog shows its first filter at first, and the
                // approved look opens on everything the cores open; then
                // each platform's, then every file.
                let mut filters = self.cores.filters();
                if let Some(all) = filters
                    .iter()
                    .position(|filter| filter.name.starts_with("All supported files"))
                {
                    let all = filters.remove(all);
                    filters.insert(0, all);
                }
                filters
                    .iter()
                    .fold(Request::new(Kind::File, "Open"), |request, filter| {
                        request.filter(&filter.name, &filter.extensions)
                    })
            }
        })
    }

    /// Take a closed file dialog's answer to the place that asked for it.
    fn poll_file_dialog(&mut self, ctx: &egui::Context) {
        if let Some((asker, answer)) = self.file_dialogs.poll(ctx) {
            if asker == FileFor::Open {
                // The worker wakes the window through `ctx` once prepared.
                if let Some(path) = answer.and_then(|paths| paths.into_iter().next()) {
                    self.open_file(path, None, ctx);
                }
                return;
            }
            self.file_chosen(asker, answer);
        }
    }

    /// Do with the chosen path what each asker did when the dialog returned
    /// it directly. An asker whose dialog closed meanwhile gets nothing.
    fn file_chosen(&mut self, asker: FileFor, answer: file_dialog::Answer) {
        let path = answer.as_ref().and_then(|paths| paths.first().cloned());
        match asker {
            FileFor::InsertImage(unit) => {
                let draft = self.insert_dialog.as_mut().filter(|d| d.unit == unit);
                if let (Some(path), Some(draft)) = (path, draft) {
                    match crate::media_dialog::browse(&path, draft.slot.geometry) {
                        Ok(entry) => draft.add_browsed(entry),
                        Err(error) => draft.error = Some(error.to_string()),
                    }
                }
            }
            FileFor::InsertJournal(unit) => {
                let draft = self.insert_dialog.as_mut().filter(|d| d.unit == unit);
                if let (Some(path), Some(draft)) = (path, draft) {
                    draft.journal_path = path.to_string_lossy().into_owned();
                }
            }
            FileFor::BlankDisk => {
                if let (Some(path), Some(form)) = (path, &mut self.blank_disk) {
                    form.output = path.to_string_lossy().into_owned();
                }
            }
            FileFor::DriveImage(unit, owner) => {
                let Some(path) = path else {
                    return;
                };
                // The image would go into another machine's drive, or its
                // Insert floppy step would replace the dialog now open.
                if self.drive_owner() != Some(owner) {
                    self.message = "The chosen image was not inserted: the machine changed while the file dialog was open".into();
                } else if self.has_dialog() {
                    self.message =
                        "The chosen image was not inserted: another dialog is open".into();
                } else {
                    self.browse_into(unit, &path);
                }
            }
            FileFor::RomImport => {
                let Some(folder) = path else {
                    return;
                };
                if self.command_allowed(Command::ImportFirmware) {
                    self.image_library
                        .begin_firmware_import(folder, self.firmware_cache.clone());
                } else {
                    self.message = if self.presenter.is_some() {
                        "ROM folder not imported: stop the machine first"
                    } else {
                        "ROM folder not imported: the image library cannot import now"
                    }
                    .into();
                }
            }
            FileFor::FirmwareSource(target, package) => {
                use crate::library_view::firmware::SourceTarget;
                // Edit metadata… may have been closed and opened for another
                // package. An import's answer goes to the draft of the same
                // catalogue set, which is the set it was picked for.
                let reopened = target == SourceTarget::Edit
                    && self.image_library.firmware_edit_tag() != package.as_deref();
                match (reopened, path) {
                    (false, path) => self.image_library.firmware_source_picked(target, path),
                    (true, Some(_)) => {
                        self.message = "The chosen source archive was not used: Edit metadata closed or shows another package".into();
                    }
                    (true, None) => {}
                }
            }
            FileFor::InstallMedia(pick) => {
                self.image_library
                    .media_picked(pick, answer.unwrap_or_default());
            }
            // The Create view's list stays with the picker, also after
            // ‹ Back to search, so the answer joins it.
            FileFor::CreateFiles | FileFor::CreateFolder => {
                if let Some(picker) = &mut self.picker {
                    picker.create.add(answer.unwrap_or_default());
                }
            }
            // Taken in `poll_file_dialog`, which has the context the worker
            // wakes the window with.
            FileFor::Open => {}
            FileFor::RomFolder => {
                let open = self
                    .dialog
                    .as_ref()
                    .is_some_and(|(title, _)| *title == "ROM folders");
                if let Some(path) = path.filter(|_| open) {
                    if let Err(error) = self.preferences_draft.add_folder(&path) {
                        self.message = format!("ROM folder: {error}");
                    }
                }
            }
        }
    }

    fn speed_status(&self) -> String {
        self.presenter
            .as_ref()
            .map_or_else(|| "Speed 100%".into(), |p| p.speed_status())
    }

    /// The status bar's message cell. An open file dialog answers later, so
    /// the cell says the shell waits for it until then, unless a second file
    /// dialog was refused meanwhile.
    fn status_message(&self) -> &str {
        // A refused second dialog says so instead, while the first waits.
        if self.file_dialogs.is_open() && !self.file_dialogs.refused() {
            "Waiting for the file dialog…"
        } else if self.presenter.as_ref().is_some_and(|p| p.mouse.captured()) {
            if self.macos_shortcuts {
                "Pointer captured — ⌘G releases"
            } else {
                "Pointer captured — Ctrl+Alt+G releases"
            }
        } else {
            &self.message
        }
    }

    fn status(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let drives = self.drive_views();
        let inserted = self.floppy_inserted();
        let now = self.elapsed();
        let message = self.status_message();
        let entry = self.selected.and_then(|index| self.library.get(index));
        // The speaker stands for the word "Audio", as in the main-window
        // mock-up's "🔊 SB16" cell, so the cell's text keeps its width.
        let audio = match &self.presenter {
            Some(p) => {
                let text = p.audio.status_text();
                format!(
                    "{} {}",
                    audio_mark(p.audio.audible()),
                    text.strip_prefix("Audio ").unwrap_or(text)
                )
            }
            None => format!("{} stopped", audio_mark(true)),
        };
        let speed = self.speed_status();
        let labels = [
            message,
            "Power",
            "",
            entry
                .filter(|entry| !entry.rate.is_empty())
                .map_or("CPU unknown", |entry| &entry.rate),
            entry.map_or("EMS unknown", |entry| &entry.ems),
            &audio,
            &speed,
        ];
        let widths = shell_model::status_widths(ui.available_width());
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
        let mut x = rect.left();
        for (index, (label, width)) in labels.into_iter().zip(widths).enumerate() {
            let cell = egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                Vec2::new(width, rect.height()),
            );
            x += width;
            if index == 2 {
                self.drive_chips(ui, cell, &drives, inserted, now, commands);
                continue;
            }
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(cell.shrink(3.0))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(cell.intersect(ui.clip_rect()));
                    if index == 0 && self.visible_discovery() && self.start_request.is_none() {
                        firmware_progress(
                            ui,
                            &self.discovery_control.snapshot(),
                            self.preferences.reduced_motion,
                        );
                    } else if index == 1 {
                        led(ui, self.presenter.is_some(), label);
                    } else {
                        ui.add(egui::Label::new(label).truncate())
                            .on_hover_text(label);
                    }
                },
            );
            bevel(ui, cell, true);
        }
    }

    /// The running machine's drives, naming where its own media came from;
    /// while no machine runs, the drives holding a floppy inserted for Start.
    fn drive_views(&mut self) -> Vec<crate::drive_chips::DriveView> {
        use crate::drive_chips::Origin;
        let pending_image = self
            .pending_media
            .as_ref()
            .filter(|pending| matches!(pending.owner, crate::media_dialog::PendingOwner::Image(_)))
            .and(self.image_library.selected.as_ref());
        let origin = match (&self.image_library.active, pending_image) {
            (Some((row, _, _)), _) | (None, Some(row)) => Origin::Image(row.reference.clone()),
            (None, None) => Origin::MachineFile(
                self.selected
                    .and_then(|index| self.library.get(index))
                    .map_or_else(
                        || {
                            self.args
                                .machine
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        },
                        |entry| entry.name.clone(),
                    ),
            ),
        };
        use crate::catalogue_menu::{set_disk, short_name};
        if self.presenter.is_none() {
            if let Some(row) = self.image_library.selected_machine_row() {
                return machine_pending_views(row, |insert| {
                    self.image_library.pending_label(insert)
                });
            }
        }
        let mut views = match &mut self.presenter {
            Some(presenter) => presenter.drive_views(&origin),
            None => self
                .pending_media
                .as_ref()
                .map_or_else(Vec::new, |pending| pending.drive_views(&origin)),
        };
        // A library disk takes the short name its catalogue gives it.
        for medium in views.iter_mut().filter_map(|view| view.medium.as_mut()) {
            let Some(source) = &medium.set else {
                continue;
            };
            if let Some(set) = self.image_library.picker_set(&source.set) {
                medium.name = short_name(set_disk(&set, &source.medium_id), &medium.name);
            }
        }
        views
    }

    /// The machines the media picker offers and the one it starts on: the
    /// running machine alone; else every stopped machine of the library
    /// with a floppy drive, the selected one first; else the selected image
    /// or machine file, whose floppies wait for Start.
    fn picker_targets(
        &mut self,
        sets: &[crate::media_picker::SetEntry],
        machine: Option<&str>,
    ) -> (Vec<crate::media_picker::Target>, usize) {
        use crate::media_picker::{Drive, Target, When};
        let find = |digest: &image_store::Digest| {
            sets.iter().find(|set| set.image.digest == *digest).cloned()
        };
        let slot_drives = |slots: &[Option<crate::media_dialog::FloppySlot>; 2],
                           inserted: [Option<usize>; 2],
                           name: &str| {
            (0u8..)
                .zip(slots)
                .filter_map(|(unit, slot)| {
                    let slot = slot.as_ref()?;
                    let now = inserted[usize::from(unit)];
                    let catalogue = crate::catalogue_menu::slot_catalogue(slot, now, name, &find);
                    let current = catalogue.iter().find(|e| e.inserted);
                    Some(Drive {
                        unit,
                        geometry: slot.geometry,
                        holds: current.map_or_else(|| "empty".into(), |e| e.name.clone()),
                        current: current.and_then(|e| e.set.clone()),
                        catalogue,
                        catalogue_note: None,
                    })
                })
                .collect::<Vec<_>>()
        };
        if self.presenter.is_some() {
            let name = self.session_name().unwrap_or("the machine").to_owned();
            let Some(presenter) = &mut self.presenter else {
                return (Vec::new(), 0);
            };
            let halted = presenter.awaits_reset();
            let inserted = [0usize, 1].map(|unit| presenter.inserted_floppy(unit));
            let drives = slot_drives(&presenter.media_slots, inserted, &name);
            return (
                vec![Target {
                    id: None,
                    name,
                    when: if halted { When::Halted } else { When::Now },
                    drives,
                    journal_unavailable: None,
                    create_unavailable: None,
                }],
                0,
            );
        }
        let rows = self.image_library.machine_rows();
        let wanted = machine
            .map(str::to_owned)
            .or_else(|| self.image_library.machine.clone());
        if let Some(wanted) = wanted {
            let mut targets = Vec::new();
            let mut at = None;
            // A machine another program runs takes no disk for its Start.
            for row in rows.iter().filter(|row| {
                !row.drives.is_empty()
                    && row.needs_newer.is_none()
                    && row.listing.state != pc_image::machine::State::Running
            }) {
                if row.id() == wanted {
                    at = Some(targets.len());
                }
                let drives = row
                    .drives
                    .iter()
                    .map(|&(unit, geometry)| {
                        let letter = crate::media_picker::drive_name(unit);
                        let pending = row
                            .listing
                            .machine
                            .pending
                            .iter()
                            .find(|p| p.drive == letter);
                        let created = row.created.iter().find(|c| c.drive == letter);
                        // The drive takes back the catalogue it kept at the
                        // last Stop, after a pending insert's disks.
                        let waiting = self.machine_waiting(row, unit);
                        let at_start = waiting
                            .chosen
                            .and_then(|index| waiting.disks.iter().find(|d| d.index == index));
                        let catalogue = if waiting.kept.is_some() {
                            waiting.disks.clone()
                        } else {
                            Vec::new()
                        };
                        Drive {
                            unit,
                            geometry,
                            holds: match (pending, created, waiting.kept) {
                                (Some(p), _, _) => {
                                    format!("{} at Start", self.image_library.pending_label(p))
                                }
                                (None, Some(c), _) => format!("{} at Start", c.title),
                                (None, None, Some(_)) => at_start.map_or_else(
                                    || "empty at Start".to_owned(),
                                    |disk| format!("{} at Start", disk.name),
                                ),
                                (None, None, None) => "as the image sets it".to_owned(),
                            },
                            current: pending
                                .and_then(|p| {
                                    let set =
                                        sets.iter().find(|s| s.image.digest == p.set.digest)?;
                                    Some((
                                        p.set.digest.clone(),
                                        set.disks.iter().position(|d| d.id == p.medium)?,
                                    ))
                                })
                                .or_else(|| at_start.and_then(|disk| disk.set.clone())),
                            catalogue_note: waiting
                                .kept
                                .is_none()
                                .then(|| IMAGE_DISKS_AT_START.into()),
                            catalogue,
                        }
                    })
                    .collect();
                targets.push(Target {
                    id: Some(row.id().to_owned()),
                    name: row.name().to_owned(),
                    when: When::AtStart,
                    drives,
                    journal_unavailable: None,
                    create_unavailable: None,
                });
            }
            // The wanted machine is the target or there is none: a disk never
            // goes silently into another machine.
            return match at {
                Some(at) => (targets, at),
                None => (Vec::new(), 0),
            };
        }
        let name = self.stopped_name();
        let drives = self
            .pending_media
            .as_ref()
            .map_or_else(Vec::new, |pending| {
                slot_drives(&pending.slots, [None, None], &name)
            });
        // An image has no workspace before Start, so its library disks go in
        // write-protected; the picker says so instead of offering a journal.
        let image_owned = matches!(
            self.pending_media.as_ref().map(|pending| &pending.owner),
            Some(crate::media_dialog::PendingOwner::Image(_))
        );
        (
            vec![Target {
                id: None,
                name,
                when: When::AtStart,
                drives,
                journal_unavailable: image_owned.then(|| {
                    "Before Start a disk goes into an image write-protected: its changes would belong to the machine Start makes.".to_owned()
                }),
                // An image keeps no disks of its own before Start makes its
                // machine, so a created disk would have nowhere to live.
                create_unavailable: image_owned.then(|| {
                    "A created disk goes into a machine: choose one of the library's machines, or start this image first.".to_owned()
                }),
            }],
            0,
        )
    }

    /// The selected image or machine file, as a stopped machine whose
    /// floppies wait for Start is named.
    fn stopped_name(&self) -> String {
        self.image_library
            .selected
            .as_ref()
            .map(|row| row.name().to_owned())
            .or_else(|| {
                self.selected
                    .and_then(|i| self.library.get(i))
                    .map(|e| e.name.clone())
            })
            .unwrap_or_else(|| "the machine".into())
    }

    /// The disk each floppy drive of the running machine holds now, by its
    /// catalogue index.
    fn floppy_inserted(&mut self) -> [Option<usize>; 2] {
        match &mut self.presenter {
            Some(presenter) => [0usize, 1].map(|unit| presenter.inserted_floppy(unit)),
            None => [None, None],
        }
    }

    /// Drive `unit`'s Catalogue submenu, from the catalogue the media
    /// picker's This machine lists; `inserted` as [`Self::floppy_inserted`]
    /// gives it. A stopped library machine and an image know their own disks
    /// only at Start: their submenu lists the disks waiting for it, for a
    /// library machine the catalogue it kept at its last Stop. `None` while
    /// the drive takes no disk.
    fn catalogue(
        &self,
        unit: u8,
        inserted: [Option<usize>; 2],
    ) -> Option<crate::catalogue_menu::Catalogue> {
        use crate::catalogue_menu::{
            entry_name, slot_catalogue, waiting_catalogue, Catalogue, Pending,
        };
        if !self
            .enablement()
            .floppy
            .get(usize::from(unit))
            .copied()
            .unwrap_or(false)
        {
            return None;
        }
        let find = |digest: &image_store::Digest| self.image_library.picker_set(digest);
        if let Some(presenter) = &self.presenter {
            let slot = presenter.media_slots.get(usize::from(unit))?.as_ref()?;
            let now = inserted[usize::from(unit)];
            let name = self.session_name().unwrap_or("the machine");
            return Some(Catalogue {
                unit,
                disks: slot_catalogue(slot, now, name, &find),
                in_drive: now,
                pending: None,
                unknown: None,
                kept: false,
            });
        }
        let waiting = |waiting: Waiting| Catalogue {
            unit,
            disks: waiting.disks,
            in_drive: None,
            pending: waiting.chosen.map(Pending::Entry),
            unknown: Some(IMAGE_DISKS_AT_START.into()),
            kept: waiting.kept.is_some(),
        };
        if self.image_library.machine.is_some() {
            let row = self.image_library.selected_machine_row();
            return Some(waiting(row.map_or_else(Waiting::default, |row| {
                self.machine_waiting(row, unit)
            })));
        }
        let pending = self.pending_media.as_ref()?;
        let slot = pending.slots.get(usize::from(unit))?.as_ref()?;
        if matches!(pending.owner, crate::media_dialog::PendingOwner::Image(_)) {
            let name = self.stopped_name();
            return Some(waiting(pending.waiting(unit).map_or_else(
                Waiting::default,
                |(entries, chosen)| Waiting {
                    disks: waiting_catalogue(entries, slot.geometry, &name, &find),
                    chosen: Some(chosen),
                    kept: None,
                },
            )));
        }
        let waiting = pending.chosen(unit).map(|entry| match entry.index {
            Some(index) => Pending::Entry(index),
            None => {
                let set = entry.source.as_ref().and_then(|source| find(&source.set));
                Pending::Joins(entry_name(entry, set.as_ref()))
            }
        });
        Some(Catalogue {
            unit,
            disks: slot_catalogue(slot, None, &self.stopped_name(), &find),
            in_drive: pending.power_on(unit),
            pending: waiting,
            unknown: None,
            kept: false,
        })
    }

    /// The disks waiting for drive `unit` of the stopped library machine
    /// `row` at its next Start and the place of the one that goes in: a
    /// pending insert's (a library set's disk, every disk of the set when all
    /// join, else the disks Create from files… made for the machine), then
    /// the catalogue the machine kept at its last Stop, without the disks the
    /// insert brings itself.
    fn machine_waiting(
        &self,
        row: &crate::library_view::machines::MachineRow,
        unit: u8,
    ) -> Waiting {
        use crate::catalogue_menu::{set_disk, short_name};
        use crate::kept_catalogue::KeptDisk;
        use crate::media_picker::CatalogueDisk;
        let letter = crate::media_picker::drive_name(unit);
        let Some(&(_, geometry)) = row.drives.iter().find(|(u, _)| *u == unit) else {
            return Waiting::default();
        };
        let words = |journal: bool| {
            if journal {
                format!("changes kept in {}", row.name())
            } else {
                "write-protected".to_owned()
            }
        };
        let listed = row
            .listing
            .machine
            .pending
            .iter()
            .find(|p| p.drive == letter);
        let created = row.created.iter().find(|c| c.drive == letter);
        let (mut disks, chosen) = if let Some(insert) = listed {
            match self.image_library.picker_set(&insert.set.digest) {
                None => {
                    let disk = CatalogueDisk {
                        name: self.image_library.pending_label(insert),
                        format: geometry,
                        write: words(insert.journal),
                        inserted: false,
                        index: 0,
                        set: None,
                    };
                    (vec![disk], Some(0))
                }
                Some(set) => {
                    let disks: Vec<CatalogueDisk> = (0..set.disks.len())
                        .filter(|at| insert.all || set.disks[*at].id == insert.medium)
                        .enumerate()
                        .map(|(index, at)| {
                            let disk = &set.disks[at];
                            CatalogueDisk {
                                name: short_name(set_disk(&set, &disk.id), &disk.id),
                                format: disk.format.unwrap_or(geometry),
                                write: words(insert.journal),
                                inserted: false,
                                index,
                                set: Some((insert.set.digest.clone(), at)),
                            }
                        })
                        .collect();
                    let chosen = disks.iter().position(|disk| {
                        disk.set
                            .as_ref()
                            .is_some_and(|(_, at)| set.disks[*at].id == insert.medium)
                    });
                    (disks, chosen)
                }
            }
        } else if let Some(created) = created {
            let disks = created
                .disks
                .iter()
                .enumerate()
                .map(|(index, disk)| CatalogueDisk {
                    name: disk.name.clone(),
                    format: geometry,
                    write: words(created.journal),
                    inserted: false,
                    index,
                    set: None,
                })
                .collect();
            (disks, Some(created.first()))
        } else {
            (Vec::new(), None)
        };
        let Some(kept) = row.kept.iter().find(|k| k.drive == letter) else {
            return Waiting {
                disks,
                chosen,
                kept: None,
            };
        };
        // The disks the pending insert brings itself are listed once, as it
        // has them.
        let brought = |disk: &KeptDisk| match disk {
            KeptDisk::Set { set, medium, .. } => listed.is_some_and(|insert| {
                insert.set.digest == set.digest && (insert.all || insert.medium == *medium)
            }),
            KeptDisk::Image { sha256, .. } => {
                listed.is_none()
                    && created.is_some_and(|c| c.disks.iter().any(|d| d.sha256 == *sha256))
            }
            KeptDisk::Own { .. } => false,
        };
        let from = disks.len();
        let find = |digest: &image_store::Digest| self.image_library.picker_set(digest);
        disks.extend(crate::kept_catalogue::waiting_disks(
            kept,
            geometry,
            row.name(),
            &find,
            from,
            &brought,
        ));
        Waiting {
            chosen: if from > 0 { chosen } else { kept.in_drive },
            disks,
            kept: Some(from),
        }
    }

    /// Make disk `index` of the disks waiting for drive `unit` of the
    /// stopped library machine `row` ([`Self::machine_waiting`]) the one that
    /// goes in at its next Start: another disk of its pending insert, or a
    /// kept disk, which takes the pending insert out.
    fn choose_waiting(
        &mut self,
        row: &crate::library_view::machines::MachineRow,
        unit: u8,
        index: usize,
    ) {
        let drive = crate::media_picker::drive_name(unit);
        let waiting = self.machine_waiting(row, unit);
        let Some(disk) = waiting.disks.iter().find(|disk| disk.index == index) else {
            return;
        };
        if let Some(from) = waiting.kept.filter(|from| index >= *from) {
            let listed = row.listing.machine.pending.iter().any(|p| p.drive == drive);
            if listed || row.created.iter().any(|c| c.drive == drive) {
                self.image_library.set_pending(row.id(), &drive, None);
            }
            self.image_library
                .set_kept_disk(row.id(), &drive, index - from);
            return;
        }
        if waiting.chosen == Some(index) {
            return;
        }
        if let Some(insert) = row
            .listing
            .machine
            .pending
            .iter()
            .find(|p| p.drive == drive)
        {
            let medium = disk.set.as_ref().and_then(|(digest, at)| {
                Some(
                    self.image_library
                        .picker_set(digest)?
                        .disks
                        .get(*at)?
                        .id
                        .clone(),
                )
            });
            if let Some(medium) = medium.filter(|medium| *medium != insert.medium) {
                let insert = image_store::machine::PendingInsert {
                    medium,
                    ..insert.clone()
                };
                self.image_library
                    .set_pending(row.id(), &drive, Some(insert));
            }
        } else if row.created.iter().any(|c| c.drive == drive) {
            self.image_library
                .set_created_first(row.id(), &drive, index);
        }
    }

    /// A disk of drive `unit`'s Catalogue submenu, by its catalogue index:
    /// what Insert of the picker's This machine does with it. Running or
    /// halted, it goes in now; stopped, it waits for Start, and the disk
    /// already in the drive takes a waiting one out, as Take out again does.
    /// Of the disks waiting for a library machine's or an image's Start,
    /// the chosen one goes in then instead.
    fn choose_catalogue_disk(&mut self, unit: u8, index: usize) {
        let inserted = self.floppy_inserted();
        let Some(catalogue) = self.catalogue(unit, inserted) else {
            return;
        };
        let Some(disk) = catalogue
            .disks
            .iter()
            .find(|disk| disk.index == index)
            .cloned()
        else {
            return;
        };
        let name = disk.name.clone();
        let drive = crate::media_picker::drive_name(unit);
        if let Some(presenter) = &self.presenter {
            let halted = presenter.awaits_reset();
            self.apply_picker(crate::media_picker::Outcome::Entry { unit, index });
            self.message = if halted && unit == 0 {
                format!("{drive} now holds {name}. Press Ctrl+Alt+Del to boot from it.")
            } else {
                format!("{drive} now holds {name}.")
            };
        } else if catalogue.unknown.is_some() {
            let machine = match self.image_library.selected_machine_row().cloned() {
                Some(row) if self.image_library.machine.is_some() => {
                    self.choose_waiting(&row, unit, index);
                    row.name().to_owned()
                }
                _ => {
                    if let Some(pending) = &mut self.pending_media {
                        pending.choose(unit, index);
                    }
                    self.stopped_name()
                }
            };
            self.message = format!("{name} goes into {drive} of {machine} at its next Start.");
        } else if catalogue.in_drive == Some(index) {
            self.message = if catalogue.pending.is_some() {
                if let Some(pending) = &mut self.pending_media {
                    pending.eject(unit);
                }
                format!("Took the pending disk out; {name} stays in {drive}.")
            } else {
                format!("{name} is in {drive} already.")
            };
        } else {
            self.apply_picker(crate::media_picker::Outcome::Entry { unit, index });
            self.message = format!(
                "{name} goes into {drive} of {} at its next Start.",
                self.stopped_name()
            );
        }
    }

    /// All N disks… of drive `unit`'s Catalogue submenu: Insert… on This
    /// machine, with the disk in the drive chosen and scrolled into view;
    /// while the drive knows only the disks waiting for Start, Insert… with
    /// their set chosen; a library machine's catalogue kept at its last Stop
    /// on This machine, with the disk waiting for Start selected.
    fn open_catalogue_picker(&mut self, unit: u8) {
        use crate::catalogue_menu::Pending;
        use crate::media_picker::{Choice, Source};
        let inserted = self.floppy_inserted();
        let Some(catalogue) = self.catalogue(unit, inserted) else {
            return;
        };
        if catalogue.unknown.is_some() && !catalogue.kept {
            let set = catalogue
                .disks
                .iter()
                .find_map(|disk| Some(disk.set.as_ref()?.0.clone()));
            self.open_picker(unit, None, set);
            return;
        }
        self.open_picker(unit, None, None);
        if let Some(picker) = &mut self.picker {
            picker.set_source(Source::Machine);
            let waiting = match catalogue.pending {
                Some(Pending::Entry(index)) if catalogue.kept => Some(index),
                _ => None,
            };
            if let Some(index) = catalogue.in_drive.or(waiting) {
                picker.choose(Choice::Entry(unit, index));
            }
            picker.reveal = true;
        }
    }

    /// Disks a library machine took at Start from sets of the library, by
    /// the pending inserts that put them in: they lead their drives'
    /// catalogues, chosen disk first, and keep the short names their sets
    /// give them after Start too.
    fn name_inserted_disks(&mut self, inserts: &[image_store::machine::PendingInsert]) {
        for insert in inserts {
            let unit = match insert.drive.as_str() {
                "A:" => 0,
                "B:" => 1,
                _ => continue,
            };
            let sources = self.image_library.inserted_sources(insert);
            let Some(slot) = self
                .presenter
                .as_mut()
                .and_then(|presenter| presenter.media_slots.get_mut(unit)?.as_mut())
            else {
                continue;
            };
            for (entry, (source, label, at, count)) in slot.entries.iter_mut().zip(sources) {
                // Only the disks the insert put in, by the names it gave them.
                if entry.source.is_some() || entry.medium.name != label {
                    break;
                }
                entry.source = Some(source);
                entry.reference = Some(insert.reference.clone());
                entry.origin = Some(crate::media_dialog::set_origin(
                    &insert.reference,
                    at,
                    count,
                ));
            }
        }
    }

    /// The catalogues a library machine took back at Start, as it kept them
    /// at its last Stop: their set disks keep the short names and tooltips
    /// their sets give them, and the tag they came from for the next Stop.
    fn restore_catalogue(&mut self, drives: Vec<crate::kept_catalogue::RestoredDrive>) {
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        for drive in drives {
            if let Some(slot) = presenter
                .media_slots
                .get_mut(usize::from(drive.unit))
                .and_then(Option::as_mut)
            {
                for set in drive.sets {
                    if let Some(entry) =
                        slot.entries.iter_mut().find(|e| e.index == Some(set.index))
                    {
                        entry.source = Some(set.source);
                        entry.reference = Some(set.reference);
                        entry.origin = Some(set.origin);
                    }
                }
            }
        }
    }

    /// Insert… of drive `unit`: the media picker, on `machine` when given
    /// and with `set` chosen.
    fn open_picker(&mut self, unit: u8, machine: Option<&str>, set: Option<image_store::Digest>) {
        // A machine of an image this release does not read takes no disk for
        // its Start (machine-images §5.6).
        if self.presenter.is_none() {
            let wanted = machine
                .map(str::to_owned)
                .or_else(|| self.image_library.machine.clone());
            let reason = match wanted {
                Some(id) => self
                    .image_library
                    .machine_rows()
                    .iter()
                    .find(|row| row.id() == id)
                    .and_then(|row| row.needs_newer.clone()),
                None if self.image_library.active.is_none() => {
                    self.image_library.selection_needs_newer()
                }
                None => None,
            };
            if let Some(reason) = reason {
                self.dialog = Some(("Insert disk", reason));
                return;
            }
        }
        let sets = self.image_library.picker_sets();
        let (targets, at) = self.picker_targets(&sets, machine);
        if targets.is_empty() {
            self.dialog = Some((
                "Insert disk",
                "That machine takes no disk now: it is gone, runs in another program or has no floppy drive.".into(),
            ));
            return;
        }
        if targets.iter().all(|t| t.drives.is_empty()) {
            self.dialog = Some(("Insert disk", "This machine has no floppy drive.".into()));
            return;
        }
        self.insert_dialog = None;
        let mut picker = crate::media_picker::Picker::new(targets, at, unit, sets, set.as_ref());
        // The configured registries, which the picker greys until remotes come.
        if let Ok((config, default)) = self.registry_accounts().settings() {
            picker.remotes = crate::media_picker::remotes(&config, &default);
        }
        self.picker = Some(picker);
    }

    fn picker_modal(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let outcome = crate::media_picker::show(ctx, picker, self.classic);
        self.read_added_items();
        if let Some(outcome) = outcome {
            self.apply_picker(outcome);
        }
    }

    /// Hand the files and folders just added to the Create view to the
    /// library worker, which reads their names and sizes.
    fn read_added_items(&mut self) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        let reads = picker.create.take_reads();
        if reads.is_empty() {
            return;
        }
        if !self.image_library.can_read_host() {
            for path in reads {
                picker.create.read_done(
                    &path,
                    Err("the image store is not open, so nothing can be read".into()),
                );
            }
            return;
        }
        for path in reads {
            self.image_library
                .send(crate::library_worker::Request::ReadHost(path));
        }
    }

    /// The Insert floppy step for one disk image file of this computer:
    /// detected geometry, write mode and compatibility, for that file alone.
    fn browse_into(&mut self, unit: u8, path: &std::path::Path) {
        let Some(slot) = self.floppy_slot(unit) else {
            return;
        };
        let mut draft = crate::media_dialog::InsertDialog::new(unit, slot);
        match crate::media_dialog::browse(path, draft.slot.geometry) {
            Ok(entry) => {
                draft.add_browsed(entry);
                draft.browsed_only = true;
                self.insert_dialog = Some(draft);
            }
            Err(error) => self.dialog = Some(("Insert disk", error.to_string())),
        }
    }

    /// Do what the media picker chose, and close it.
    fn apply_picker(&mut self, outcome: crate::media_picker::Outcome) {
        use crate::media_picker::Outcome;
        // Add files… and Add folder… keep the picker open behind the dialog.
        match outcome {
            Outcome::AddFiles => return self.ask_file(FileFor::CreateFiles),
            Outcome::AddFolder => return self.ask_file(FileFor::CreateFolder),
            _ => {}
        }
        let target = self
            .picker
            .take()
            .and_then(|picker| picker.machine().id.clone());
        match outcome {
            Outcome::Cancel | Outcome::AddFiles | Outcome::AddFolder => {}
            Outcome::Create(order) => self.create_disks(*order, target),
            Outcome::Pending { id, insert } => {
                let Some(row) = self.image_library.machine_rows().iter().find(|r| r.id() == id).cloned() else {
                    self.dialog = Some(("Insert disk", "That machine is no longer in the library.".into()));
                    return;
                };
                // Another Insert into the same drive replaces its pending
                // disk; the machine's other drives keep theirs.
                self.message = format!(
                    "{} goes into {} of {} at its next Start",
                    self.image_library.pending_label(&insert),
                    insert.drive,
                    row.name()
                );
                let drive = insert.drive.clone();
                self.image_library.set_pending(&id, &drive, Some(insert));
            }
            Outcome::Set {
                unit,
                set,
                first,
                all,
                journal,
                ..
            } => {
                let Some(slot) = self.floppy_slot(unit) else {
                    return;
                };
                let mut draft = crate::media_dialog::InsertDialog::new(unit, slot);
                draft.automatic = true;
                self.insert_dialog = Some(draft);
                let store_root = self
                    .image_library
                    .root
                    .clone()
                    .or_else(|| self.preferences.store_root.clone());
                self.open_insert_library_at(store_root, Some(set.digest));
                if let Some(library) = self.insert_dialog.as_mut().and_then(|d| d.library.as_mut()) {
                    library.first = first;
                    library.load_all = all;
                    if library.journal_unavailable.is_none() {
                        library.journal = journal;
                    }
                }
                self.message = "Verifying installation media…".into();
            }
            Outcome::Entry { unit, index } => {
                if let Some(presenter) = &mut self.presenter {
                    presenter
                        .media_events
                        .push((SlotId(unit), MediaAction::Insert(index)));
                } else if let Some(id) = target {
                    // A disk waiting for a stopped library machine's Start:
                    // it goes in then.
                    let Some(row) = self.image_library.machine_rows().iter().find(|r| r.id() == id).cloned() else {
                        self.dialog = Some(("Insert disk", "That machine is no longer in the library.".into()));
                        return;
                    };
                    let waiting = self.machine_waiting(&row, unit);
                    let drive = crate::media_picker::drive_name(unit);
                    if let Some(disk) = waiting.disks.iter().find(|disk| disk.index == index) {
                        self.message = format!(
                            "{} goes into {drive} of {} at its next Start.",
                            disk.name,
                            row.name()
                        );
                    }
                    self.choose_waiting(&row, unit, index);
                } else if let Some(entry) = self
                    .floppy_slot(unit)
                    .and_then(|slot| slot.entries.into_iter().find(|e| e.index == Some(index)))
                {
                    if let Some(pending) = &mut self.pending_media {
                        self.message = format!(
                            "{}: {} is in the drive when Start boots",
                            crate::media_picker::drive_name(unit),
                            entry.medium.name
                        );
                        pending.insert(unit, vec![entry], 0);
                    }
                }
            }
            // The system file dialog at once, then the chosen file's own
            // Insert floppy step.
            Outcome::Browse(unit) => match (self.floppy_slot(unit), self.drive_owner()) {
                (Some(_), Some(owner)) => self.ask_file(FileFor::DriveImage(unit, owner)),
                (None, _) if self.image_library.machine.is_some() => {
                    self.dialog = Some((
                        "Insert disk",
                        "A disk image file goes into a running machine or a machine file. For a stopped machine, choose a set of this library; Import installation media… adds an image file to it.".into(),
                    ))
                }
                // A drive with a catalogue always has an owner.
                _ => {}
            },
        }
    }

    /// Create and insert: the library worker builds the disks and stores
    /// them as the machine's own. The stopped library machine `target`
    /// takes them at its next Start; a running machine, or a stopped machine
    /// file before its Start, takes them once they are made.
    fn create_disks(&mut self, order: crate::created_media::CreateOrder, target: Option<String>) {
        use crate::created_media::Place;
        if !self.image_library.can_read_host() {
            self.dialog = Some((
                "Create from files",
                "The disks were not created: the image store is not open.".into(),
            ));
            return;
        }
        let place = match target {
            Some(id) => Place::Machine(id),
            None => match self.library_media_dir() {
                Ok(media) => Place::Folder(media),
                Err(error) => {
                    self.dialog = Some(("Create from files", error));
                    return;
                }
            },
        };
        self.creating = self.drive_owner();
        self.message = "Creating the disks from files…".into();
        self.image_library
            .send(crate::library_worker::Request::CreateDisks(
                Box::new(order),
                place,
            ));
    }

    /// Insert what Create and insert made, into the machine that asked for
    /// it; a stopped library machine already has it as a pending insert.
    fn disks_created(&mut self, result: Result<Box<crate::created_media::Created>, String>) {
        use crate::created_media::Place;
        let owner = self.creating.take();
        let created = match result {
            Ok(created) => created,
            Err(error) => {
                self.message = "The disks were not created".into();
                self.dialog = Some(("Create from files", error));
                return;
            }
        };
        let order = &created.order;
        let unit = order.unit;
        let count = created.disks.len();
        let drive = crate::media_picker::drive_name(unit);
        let what = if count > 1 {
            format!("{} ({count} disks)", order.title)
        } else {
            order.title.clone()
        };
        let kept = match (&created.kept, &created.keep_failed) {
            (Some(tag), _) => format!(" Also kept in the library as {tag}."),
            (None, Some(why)) => format!(" It was not kept in the library: {why}"),
            (None, None) => String::new(),
        };
        let all = if count > 1 {
            format!("; all {count} disks join the catalogue")
        } else {
            String::new()
        };
        match &created.place {
            Place::Machine(id) => {
                let name = self
                    .image_library
                    .machine_rows()
                    .iter()
                    .find(|row| row.id() == id)
                    .map_or_else(|| id.clone(), |row| row.name().to_owned());
                self.message =
                    format!("{what} goes into {drive} of {name} at its next Start{all}.{kept}");
            }
            Place::Folder(_) => {
                // The disks go only into the drives of the machine that
                // asked for them.
                if owner.is_none() || self.drive_owner() != owner {
                    self.message = format!(
                        "{what} was created, but the machine changed meanwhile, so it was not inserted.{kept}"
                    );
                    return;
                }
                let Some(slot) = self.floppy_slot(unit) else {
                    self.message = format!(
                        "{what} was created, but {drive} is gone, so it was not inserted.{kept}"
                    );
                    return;
                };
                let entries = crate::created_media::entries(&created, slot.journal_retention);
                let name = self.session_name().unwrap_or("the machine").to_owned();
                if let Some(presenter) = &mut self.presenter {
                    let (assembled, config, slots) = presenter.media_parts();
                    match crate::media_dialog::append_entries(
                        assembled, config, slots, unit, &entries,
                    ) {
                        Ok(indices) => {
                            presenter
                                .media_events
                                .push((SlotId(unit), MediaAction::Insert(indices[0])));
                            let next = if count > 1 {
                                format!(
                                    " {} swaps to the next disk.",
                                    if unit == 0 {
                                        "Ctrl+Alt+F"
                                    } else {
                                        "Ctrl+Alt+Shift+F"
                                    }
                                )
                            } else {
                                String::new()
                            };
                            self.message =
                                format!("{what} is in {drive} of {name} now.{next}{kept}");
                        }
                        Err(error) => {
                            self.message = "The created disks did not go in".into();
                            self.dialog = Some(("Create from files", error));
                        }
                    }
                } else if let Some(pending) = &mut self.pending_media {
                    pending.insert(unit, entries, 0);
                    self.message =
                        format!("{what} goes into {drive} of {name} at its next Start{all}.{kept}");
                }
            }
        }
    }

    /// One chip per drive inside the status bar's fixed drive area: the image,
    /// an activity light, a tooltip with its details and a click menu. While
    /// no machine runs, only the floppies inserted for Start have a chip.
    fn drive_chips(
        &self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        drives: &[crate::drive_chips::DriveView],
        inserted: [Option<usize>; 2],
        now: std::time::Duration,
        commands: &mut Vec<Command>,
    ) {
        const GAP: f32 = 2.0;
        const PAD: f32 = 6.0;
        const LED: Vec2 = Vec2::new(10.0, 6.0);
        const LED_GAP: f32 = 5.0;
        if drives.is_empty() {
            bevel(ui, area, true);
            return;
        }
        let font = egui::TextStyle::Body.resolve(ui.style());
        let measure = |text: String| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), Color32::PLACEHOLDER)
                .size()
                .x
        };
        let parts: Vec<(String, String, String)> = drives
            .iter()
            .map(crate::drive_chips::DriveView::chip_parts)
            .collect();
        let chrome = 2.0 * PAD + LED.x + LED_GAP;
        let natural: Vec<f32> = parts
            .iter()
            .map(|(head, name, tail)| measure(format!("{head}{name}{tail}")) + chrome)
            .collect();
        // The format and write mode stay whole; only the name gives way.
        let minimum: Vec<f32> = parts
            .iter()
            .zip(&natural)
            .map(|((head, name, tail), natural)| {
                if name.is_empty() {
                    *natural
                } else {
                    measure(format!("{head}…{tail}")) + chrome
                }
            })
            .collect();
        let gaps = GAP * (drives.len() - 1) as f32;
        let widths =
            crate::drive_chips::chip_widths((area.width() - gaps).max(0.0), &natural, &minimum);
        let lights = self.presenter.as_ref().map(|p| &p.drive_lights);
        let mut x = area.left();
        for (((view, (head, name, tail)), width), natural) in
            drives.iter().zip(parts).zip(widths).zip(natural)
        {
            let text = view.chip_text();
            let room = (width - chrome).max(0.0);
            let shown = if natural <= width || name.is_empty() {
                text.clone()
            } else {
                let mut job = egui::text::LayoutJob::single_section(
                    name,
                    egui::TextFormat::simple(font.clone(), Color32::PLACEHOLDER),
                );
                job.wrap = egui::text::TextWrapping::truncate_at_width(
                    (room - measure(format!("{head}{tail}"))).max(0.0),
                );
                let short = ui
                    .painter()
                    .layout_job(job)
                    .rows
                    .first()
                    .map(|placed| placed.row.text())
                    .unwrap_or_default();
                format!("{head}{short}{tail}")
            };
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, area.top()),
                Vec2::new(width, area.height()),
            );
            x += width + GAP;
            let lit = lights.is_some_and(|lights| lights.lit(view.slot(), now));
            let response = ui.interact(
                rect,
                ui.scope_id().with(("drive-chip", view.slot())),
                Sense::click(),
            );
            let open =
                egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));
            let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            if response.hovered() || open {
                painter.rect_filled(
                    rect.shrink(1.0),
                    0.0,
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            let led_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + PAD, rect.center().y - LED.y / 2.0),
                LED,
            );
            painter.rect_filled(led_rect, 0.0, disk_led_color(ui, lit));
            let color = if view.medium.is_none() {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            };
            // Only when even the format and mode do not fit is the whole line cut.
            let mut job = egui::text::LayoutJob::single_section(
                shown,
                egui::TextFormat::simple(font.clone(), color),
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let galley = ui.painter().layout_job(job);
            painter.galley(
                egui::pos2(
                    led_rect.right() + LED_GAP,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                color,
            );
            bevel(ui, rect, true);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::Role::Button,
                    true,
                    if lit {
                        format!("Drive {text}, active")
                    } else {
                        format!("Drive {text}")
                    },
                )
            });
            let response = if open {
                response
            } else {
                response.on_hover_ui(|ui| drive_tooltip(ui, view, lit))
            };
            egui::Popup::menu(&response).show(|ui| self.drive_menu(ui, view, inserted, commands));
        }
        if let Some(wait) = lights.and_then(|lights| lights.next_off(now)) {
            ui.ctx().request_repaint_after(wait);
        }
    }

    /// A drive chip's menu: the Media menu's own commands for that drive.
    fn drive_menu(
        &self,
        ui: &mut egui::Ui,
        view: &crate::drive_chips::DriveView,
        inserted: [Option<usize>; 2],
        commands: &mut Vec<Command>,
    ) {
        ui.add_enabled(false, egui::Label::new(view.menu_label()));
        let Some(unit) = view.floppy_unit() else {
            ui.add_enabled(
                false,
                egui::Button::new((ui_kit::shell_icons::mark(""), view.fixed_eject_label())),
            );
            self.marked_item(ui, "", "Disk layers…", "", Command::Layers, commands);
            if let Some(label) = view.write_label() {
                ui.separator();
                ui.add_enabled(false, egui::Label::new(label));
            }
            return;
        };
        let drive = view.letter();
        ui.add_enabled_ui(view.medium.is_some(), |ui| {
            self.marked_item(
                ui,
                EJECT_MARK,
                &if view.at_start {
                    "Take out again (pending)".to_owned()
                } else {
                    format!("Eject {drive}:")
                },
                eject_shortcut(unit),
                Command::Eject(unit),
                commands,
            );
        });
        self.marked_item(
            ui,
            "",
            "Insert…",
            self.insert_shortcut(unit),
            Command::Insert(unit),
            commands,
        );
        self.marked_item(
            ui,
            "",
            "Create from files…",
            "",
            Command::CreateFromFiles(unit),
            commands,
        );
        // The chip's menu opens upward from the status bar, and so does the
        // submenu.
        self.catalogue_item(ui, unit, inserted, true, commands);
        if view.at_start {
            ui.add_enabled(
                false,
                egui::Label::new("Stopped: a disk inserted now goes in at the next Start."),
            );
        }
        if let Some(label) = view.write_label() {
            ui.separator();
            ui.add_enabled(false, egui::Label::new(label));
            if view
                .medium
                .as_ref()
                .is_some_and(|medium| medium.fixed_by.is_none())
            {
                self.marked_item(
                    ui,
                    "",
                    "Change write mode…",
                    "",
                    Command::WriteMode(unit),
                    commands,
                );
            }
        }
    }

    fn insert_shortcut(&self, unit: u8) -> &'static str {
        if unit == 0 {
            "Ctrl+Alt+I"
        } else if self.macos_shortcuts {
            "Ctrl+Alt+Shift+I"
        } else {
            ""
        }
    }

    /// Preselect the inserted floppy in a just-opened Insert dialog, so its
    /// write-mode choice applies to that disk.
    fn select_inserted(&mut self, unit: u8) {
        let index = self
            .presenter
            .as_mut()
            .and_then(|presenter| presenter.inserted_floppy(usize::from(unit)));
        if let (Some(index), Some(draft)) = (index, &mut self.insert_dialog) {
            let row = crate::drive_chips::entry_position(&draft.slot, index);
            draft.select(row);
        }
    }

    /// `machine_given`: the command line named the machine with `--machine`.
    fn new(
        args: Args,
        machine_given: bool,
        events: Rc<RefCell<Events>>,
        errors: Rc<RefCell<Option<String>>>,
        window: Option<Arc<Window>>,
        default_firmware_roots: Vec<machine_file::firmware_discovery::FirmwareRoot>,
        cores: core_launch::Cores,
    ) -> Self {
        let root = crate::machines_dir();
        // Finding the path may copy the file from the project's former
        // directory. When that fails, the status bar says so: the shell then
        // keeps defaults and saves nothing, so the old file is copied next time.
        let (preferences_path, path_error) = match image_store::location::preferences::path() {
            Ok(path) => (Some(path), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let (preferences, preference_error) = match preferences_path
            .as_ref()
            .map(|path| crate::preferences::Preferences::load(path))
            .transpose()
        {
            Ok(preferences) => (preferences.unwrap_or_default(), None),
            Err(error) => (
                crate::preferences::Preferences::default(),
                Some(error.to_string()),
            ),
        };
        let (mut paths, mut message) =
            match root.as_deref().map(shell_model::machine_paths).transpose() {
                Ok(paths) => (paths.unwrap_or_default(), "Ready".into()),
                Err(error) => (Vec::new(), error.to_string()),
            };
        if let Some(error) = path_error.clone().or(preference_error) {
            message = format!("Preferences: {error}");
        }
        let selected = shell_model::select_machine(&mut paths, &args.machine, machine_given);
        let library = paths.into_iter().map(LibraryEntry::pending).collect();
        let discovery_roots = preferences.roots(&args.rom_dirs, &default_firmware_roots);
        let mut shell = Self {
            args,
            library,
            image_library: crate::library_view::LibraryView::new(
                preferences
                    .store_root
                    .clone()
                    .or_else(|| crate::library_view::default_store_path().ok()),
            )
            .with_compatibility(
                platforms::compatibility(env!("CARGO_PKG_VERSION")).unwrap_or_default(),
            ),
            retain_workspace_on_stop: false,
            record_exceptions: false,
            exit_requested: false,
            store_path_input: String::new(),
            selected,
            presenter: None,
            paused: false,
            classic: true,
            toolbar: true,
            statusbar: true,
            fullscreen: false,
            integer_scale: false,
            dialog: None,
            error_report: None,
            message,
            texture: None,
            logo: None,
            events,
            errors,
            window,
            screen: egui::Rect::NOTHING,
            guest_focus: false,
            pointer: egui::Pos2::ZERO,
            modifiers: [false; 8],
            macos_shortcuts: cfg!(target_os = "macos"),
            host_modifiers: ModifiersState::empty(),
            fast_forward_key: None,
            consumed: BTreeSet::new(),
            held: BTreeSet::new(),
            guest_held: BTreeSet::new(),
            session: 0,
            modal_paused: false,
            hidden: false,
            minimized: false,
            open_path: String::new(),
            preferences_draft: preferences.clone(),
            preferences,
            preferences_path,
            preferences_path_error: path_error,
            rom_folder_input: String::new(),
            library_revision: 0,
            readiness: None,
            prepared: Default::default(),
            discovery_session: Default::default(),
            discovery_roots,
            start_request: None,
            checked_selection: None,
            insert_dialog: None,
            picker: None,
            pending_media: None,
            blank_disk: None,
            settings: None,
            preferences_dialog: None,
            credential_store: image_registry::credential_store::detect(),
            discovery_warnings: BTreeSet::new(),
            discovery_control: Default::default(),
            firmware_cache: machine_file::firmware_discovery::firmware_cache_path(),
            default_store_root: image_store::location::default_store_root().ok(),
            default_firmware_roots,
            rescan: false,
            refresh_listing: false,
            discovery_finished: false,
            stop_prompt: None,
            stop_prompt_input: (false, false),
            restore_input: None,
            transfer_prompt: None,
            system_keys: crate::system_keys::SystemKeys::platform(),
            clipboard: Box::<ui_kit::text_clipboard::SystemClipboard>::default(),
            file_dialogs: file_dialog::FileDialogs::native(),
            creating: None,
            cores: Arc::new(cores),
            host_dirs: crate::host_dirs(),
            player: None,
            player_commands: Vec::new(),
            opening: None,
            opened: None,
            open_refusal: None,
            #[cfg(test)]
            player_log: Vec::new(),
        };
        // Remotes lists the registries of Preferences › Registries; nothing
        // contacts them until a node opens.
        let accounts = shell.registry_accounts();
        shell.image_library.attach_remotes(accounts);
        shell
    }

    fn state(&self) -> State {
        if let Some(player) = &self.player {
            if player.paused() {
                State::Paused
            } else {
                State::Running
            }
        } else if self.presenter.is_none() {
            State::Stopped
        } else if self.paused {
            State::Paused
        } else {
            State::Running
        }
    }

    /// The stopped library selection is an image, or a machine of one, that
    /// this release does not read: nothing that reads its config is offered.
    fn selection_is_newer(&self) -> bool {
        self.presenter.is_none()
            && self.image_library.active.is_none()
            && self.image_library.selection_needs_newer().is_some()
    }

    fn command_allowed(&self, command: Command) -> bool {
        if let Some(player) = &self.player {
            return self.player_allows(player, command);
        }
        if command == Command::Open {
            return self.opens_files();
        }
        // Several library rows selected: Push to… takes them, and no command
        // of one item acts on a row left selected behind them.
        if self.image_library.several_selected() && single_item(command) {
            return false;
        }
        self.enablement().allows(command)
            && match command {
                Command::GamingMode | Command::ImageStart | Command::ImageGaming => {
                    self.image_library.selected.is_some()
                }
                Command::Rename => self.image_library.machine.is_some(),
                Command::Duplicate
                | Command::DeleteMachine
                | Command::ResetMachine
                | Command::ShowSource => {
                    self.presenter.is_none()
                        && self.image_library.machine.is_some()
                        && !self.image_library.busy
                }
                Command::DeleteSelection => {
                    self.presenter.is_none()
                        && self.image_library.selected.is_some()
                        && !self.image_library.busy
                }
                // Nothing reads the config of an image this release does not
                // read (machine-images §5.6).
                Command::NewFromImage | Command::ImageSettings => {
                    self.presenter.is_none()
                        && self.image_library.selected.is_some()
                        && !self.image_library.busy
                        && !self.selection_is_newer()
                }
                Command::ImageLayers => {
                    self.image_library.selected.is_some() && !self.selection_is_newer()
                }
                Command::Settings | Command::Layers => !self.selection_is_newer(),
                Command::ImportImage => {
                    self.presenter.is_none() && self.selected.is_some() && !self.image_library.busy
                }
                Command::DeleteTag => {
                    self.presenter.is_none()
                        && (self.image_library.selected.is_some()
                            || self.image_library.can_delete_package())
                        && self.image_library.machine.is_none()
                        && !self.image_library.busy
                }
                Command::ImportFirmware => {
                    self.presenter.is_none() && self.image_library.can_import_firmware()
                }
                Command::EditFirmware => {
                    self.presenter.is_none() && self.image_library.can_edit_firmware()
                }
                Command::SaveImage => {
                    self.image_library.active.is_some()
                        || (self.image_library.selected.is_some() && !self.selection_is_newer())
                }
                // A transfer runs on the library worker, which serves the
                // running machine's session until it stops.
                // Push to… takes the library items selected, Push here…
                // the Remotes node shown; machines are never pushed.
                Command::Push => {
                    self.presenter.is_none()
                        && self.image_library.active.is_none()
                        && self.image_library.target_needs_newer().is_none()
                        && self.image_library.can_push_to()
                }
                Command::PushHere => {
                    self.presenter.is_none()
                        && self.image_library.active.is_none()
                        && self.image_library.can_push_here()
                }
                Command::PullReference | Command::GetTemplates => {
                    self.presenter.is_none() && self.image_library.active.is_none()
                }
                // Pull… takes the tags selected in Remotes.
                Command::Pull => {
                    self.presenter.is_none()
                        && self.image_library.active.is_none()
                        && self.image_library.can_pull_remote()
                }
                // The library, and with it Remotes, shows only while no
                // machine runs.
                Command::RefreshRemote => {
                    self.presenter.is_none() && self.image_library.remote_shown()
                }
                Command::ImportMedia => {
                    self.presenter.is_none() && self.image_library.can_import_media()
                }
                Command::InsertSet(_) => self.image_library.selected_set().is_some(),
                // A halted guest waits for a reset; Resume would only hide that.
                Command::Pause => !self.presenter.as_ref().is_some_and(|p| p.halted),
                Command::Eject(unit) => {
                    self.presenter.is_some()
                        || self
                            .pending_media
                            .as_ref()
                            .is_some_and(|pending| pending.has_insert(unit))
                        || self
                            .image_library
                            .selected_machine_row()
                            .is_some_and(|row| {
                                let drive = crate::media_picker::drive_name(unit);
                                row.listing.machine.pending.iter().any(|p| p.drive == drive)
                                    || row.created.iter().any(|c| c.drive == drive)
                            })
                }
                _ => true,
            }
    }

    /// What a command does while a machine started from a file runs. The
    /// commands that meet a running machine ask the player; the PC's own
    /// (Insert…, Layers, Save as image…, Settings, Start) wait until it
    /// stops; the rest stay as they are while the PC runs.
    fn player_allows(&self, player: &player_ui::Player, command: Command) -> bool {
        use Command::*;
        match command {
            Pause => player.running(),
            Reset => player.offers().reset,
            SaveState | LoadState => player.offers().saved_state,
            Stop | PowerCycle | Mute | Speed(_) | FastForward | Release | Fullscreen => true,
            // The media keys work the first two removable slots.
            Next(unit) | Eject(unit) => player.offers().media.len() > usize::from(unit),
            Open => self.opens_files(),
            Start | GamingMode | ImageStart | ImageGaming | Layers | ImageLayers | SaveImage
            | Settings | Insert(_) | CreateFromFiles(_) | InsertFile(_) | InsertSet(_)
            | WriteMode(_) | CatalogueDisk(..) | CatalogueAll(_) => false,
            Library | Exit | Minimize | Shortcuts | About | Preferences | Placeholder(_)
            | FirmwareFolders | CreateBlank | StoreFolder => true,
            // The library's commands, which wait for a stopped machine as
            // they do while the PC runs.
            _ => false,
        }
    }

    /// Whether a compiled-in core opens files: one lists a kind of content.
    /// The PC starts only from its machines, so a release with the PC
    /// alone keeps File › Open… greyed.
    fn opens_files(&self) -> bool {
        self.cores.usable().any(|core| {
            core.description()
                .platforms
                .iter()
                .any(|platform| platform.content.iter().any(|kind| kind.is_valid()))
        })
    }

    /// Tools › Preferences… on its Registries page. Log in… of a registry in
    /// Remotes opens it on that registry's Log in dialog, or on Add
    /// registry… for it when it has no entry: never on another registry's
    /// row.
    fn open_preferences(&mut self) {
        self.release();
        let mut dialog = crate::preferences_sheet::PreferencesDialog::new(
            self.preferences_path.clone(),
            self.credential_store.clone(),
        );
        if self.macos_shortcuts {
            let on = self.preferences.send_system_shortcuts;
            // Reopening Preferences keeps telling what is still missing.
            let note = (on && self.system_keys.access_missing()).then(|| ACCESS_NOTE.to_owned());
            dialog.system_shortcuts = Some(crate::preferences_sheet::SystemShortcuts { on, note });
        }
        if let Some(host) = self.image_library.login_wanted.take() {
            let page = &mut dialog.registries;
            match page.rows.iter().position(|row| row.entry.host == host) {
                Some(index) => {
                    page.select(index);
                    page.open_login();
                }
                None => page.open_add_for(&host),
            }
        }
        self.preferences_dialog = Some(dialog);
    }

    /// Commands the library's dialogs asked for: Import ROM folder… of the
    /// New machine wizard, and Preferences for a Log in… of a registry in
    /// Remotes.
    fn library_commands(&mut self, commands: &mut Vec<Command>) {
        if std::mem::take(&mut self.image_library.firmware_import_wanted) {
            commands.push(Command::ImportFirmware);
        }
        if let Some((machine, unit, set)) = self.image_library.picker_wanted.take() {
            self.open_picker(unit, Some(&machine), Some(set));
        }
        if self.image_library.login_wanted.is_some()
            || std::mem::take(&mut self.image_library.registries_wanted)
        {
            commands.push(Command::Preferences);
        }
    }

    /// A registry in Remotes lists again after a Log in… that opened
    /// Preferences.
    fn close_preferences(&mut self) {
        self.preferences_dialog = None;
        self.image_library.preferences_closed();
    }

    /// Logins for Image › Push… and Pull…: the shared preferences file and
    /// this session's credential store, as Preferences › Registries uses them.
    fn registry_accounts(&self) -> crate::registry_transfer::Accounts {
        crate::registry_transfer::Accounts {
            path: self.preferences_path.clone(),
            store: self.credential_store.clone(),
            env: image_registry::credentials::EnvCredentials::from_env(),
            default_override: std::env::var(image_registry::config::DEFAULT_REGISTRY_ENV).ok(),
        }
    }

    fn enablement(&self) -> Enablement {
        let entry = self.selected.and_then(|index| self.library.get(index));
        Enablement {
            state: self.state(),
            ready: if self.image_library.selected.is_some() {
                !self.image_library.busy
                    && (self.image_library.ready() || self.image_library.readiness.is_none())
            } else {
                !self.image_library.busy
                    && entry.is_some_and(|entry| entry.checked && entry.warning.is_none())
            },
            floppy: match &self.presenter {
                Some(presenter) if presenter.takes_media() => {
                    presenter.media_slots.each_ref().map(Option::is_some)
                }
                Some(_) => [false; 2],
                // A machine of an image this release does not read takes no
                // disk: it cannot start here (machine-images §5.6).
                None if self.selection_is_newer() => [false; 2],
                None => match self.image_library.selected_machine_row() {
                    // A stopped machine takes disks for its next Start.
                    Some(row) => [0u8, 1].map(|unit| row.drives.iter().any(|(u, _)| *u == unit)),
                    None => self.pending_media.as_ref().map_or([false; 2], |pending| {
                        pending.slots.each_ref().map(Option::is_some)
                    }),
                },
            },
            audio: self.presenter.as_ref().is_some_and(|p| p.audio.available()),
        }
    }

    fn sync_routing(&self) {
        let mut events = self.events.borrow_mut();
        events.guest_focus = self.guest_focus;
        events.captured = self
            .presenter
            .as_ref()
            .is_some_and(|presenter| presenter.mouse.captured())
            || self
                .player
                .as_ref()
                .is_some_and(player_ui::Player::pointer_captured);
        events.window = self.window.clone();
    }

    fn release_pointer(&mut self) {
        if let Some(presenter) = &mut self.presenter {
            presenter.release_pointer();
        }
        if let Some(player) = &mut self.player {
            player.release_pointer();
        }
        self.sync_routing();
    }

    /// Let go of Hold for Unlimited, of whichever machine runs.
    fn end_fast_forward(&mut self) {
        let elapsed = self.elapsed();
        if let Some(presenter) = &mut self.presenter {
            presenter.hold_fast_forward(false, elapsed);
        }
        if let Some(player) = &mut self.player {
            player.command(
                player_ui::Command::FastForward(false),
                elapsed,
                &mut io::stderr(),
            );
        }
    }

    fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        if self.host_modifiers.super_key() && !modifiers.super_key() {
            self.end_fast_forward();
            self.fast_forward_key = None;
        }
        // Guest focus resets must not erase modifiers still held by the host.
        self.host_modifiers = modifiers;
    }

    fn reset_keyboard(&mut self) {
        self.end_fast_forward();
        if let Some(presenter) = &mut self.presenter {
            presenter.focus_changed(false);
        }
        if let Some(player) = &mut self.player {
            player.focus_changed(false);
        }
        self.modifiers = [false; 8];
        self.fast_forward_key = None;
        self.consumed.clear();
        self.held.clear();
        self.guest_held.clear();
    }

    fn focus_guest(&mut self) {
        if !self.guest_focus {
            self.reset_keyboard();
            self.guest_focus = true;
            if let Some(presenter) = &mut self.presenter {
                presenter.focus_changed(true);
            }
            if let Some(player) = &mut self.player {
                player.focus_changed(true);
            }
        }
        self.sync_routing();
    }

    fn release(&mut self) {
        self.reset_keyboard();
        self.guest_focus = false;
        self.sync_routing();
        self.sync_system_keys();
    }

    /// Whether the guest takes macOS system shortcuts now: the user turned
    /// the option on and a running guest has the keyboard. Pause, dialogs and
    /// a hidden window give the shortcuts back to macOS.
    fn system_keys_wanted(&self) -> bool {
        self.macos_shortcuts
            && self.preferences.send_system_shortcuts
            && self.guest_focus
            && !self.paused
            && !self.hidden
            && !self.minimized
            && !self.has_dialog()
            && (self
                .presenter
                .as_ref()
                .is_some_and(|presenter| presenter.running && presenter.error.is_none())
                || self
                    .player
                    .as_ref()
                    .is_some_and(|player| player.running() && !player.paused()))
    }

    fn sync_system_keys(&mut self) {
        let wanted = self.system_keys_wanted();
        self.events.borrow_mut().system_keys = wanted;
        let events = self.events.clone();
        if let Some(hint) = self.system_keys.sync(wanted, || system_key_sink(events)) {
            self.message = hint.into();
        }
    }

    /// Preferences › General: the option applies and saves at once. Turning
    /// it on is the only moment macOS is asked for Accessibility access.
    fn set_system_shortcuts(&mut self, on: bool) {
        self.preferences.send_system_shortcuts = on;
        self.preferences_draft.send_system_shortcuts = on;
        let saved = self
            .preferences_file()
            .and_then(|path| self.preferences.save(path));
        // The option is on for this session even when saving failed, so it
        // asks for access either way.
        let granted = !on || self.system_keys.request_access();
        let note = [
            saved
                .err()
                .map(|error| format!("Cannot save preferences: {error}")),
            (!granted).then(|| ACCESS_NOTE.to_owned()),
        ]
        .into_iter()
        .flatten()
        .reduce(|first, second| format!("{first} {second}"));
        if let Some(shortcuts) = self
            .preferences_dialog
            .as_mut()
            .and_then(|dialog| dialog.system_shortcuts.as_mut())
        {
            shortcuts.on = on;
            shortcuts.note = note;
        }
        self.sync_system_keys();
    }

    /// Ctrl+Alt+Del of a halted machine is its reset button: POST runs again
    /// in the same session, with RAM and the inserted media kept.
    fn reset_halted(&mut self) {
        let elapsed = self.elapsed();
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        if let Err(error) = presenter.reset_halted(&mut io::stderr()) {
            self.show_error(error.to_string());
            return;
        }
        // The next frame resumes the run as it does after a modal pause, so
        // host pacing restarts from that frame's time.
        presenter.occlusion_changed(true, elapsed);
        self.modal_paused = true;
        self.paused = false;
        self.message = "Machine reset — POST runs again".into();
    }

    fn focus_lost(&mut self) {
        self.release();
    }

    fn elapsed(&self) -> std::time::Duration {
        if let Some(player) = &self.player {
            return player.elapsed();
        }
        self.presenter
            .as_ref()
            .and_then(|presenter| presenter.started)
            .map_or(std::time::Duration::ZERO, |started| started.elapsed())
    }

    fn abandon_start(&mut self) {
        self.start_request = None;
        if self.message == "Loading machine…" {
            self.message = "Ready".into();
        }
    }

    fn start(&mut self) {
        if self.image_library.selected.is_some() {
            if self.presenter.is_none() {
                self.image_library
                    .launch(crate::library_worker::WorkspaceKind::Regular);
            }
            return;
        }
        if self.presenter.is_some() {
            return;
        }
        // A machine file starts without the library worker, but nothing it
        // loads should wait for a registry either.
        self.image_library.stop_remotes();
        self.reset_keyboard();
        if let Some(index) = self.selected {
            self.start_request = Some(index);
            self.library[index].checked = false;
            self.library[index].warning = Some("Loading machine…".into());
            self.message = "Loading machine…".into();
            *self.errors.borrow_mut() = None;
        }
    }

    fn apply_readiness(
        &mut self,
        index: usize,
        revision: u64,
        prepared: shell_model::PreparedEntry,
    ) {
        if revision != self.library_revision {
            return;
        }
        let (entry, loaded) = prepared;
        self.discovery_warnings
            .extend(entry.discovery_warnings.iter().cloned());
        self.library[index] = entry;
        if self.selected != Some(index) || self.start_request != Some(index) {
            if let Some(loaded) = loaded {
                self.prepared.insert(index, loaded);
            }
            return;
        }
        self.start_request = None;
        if let Some(loaded) = loaded {
            self.args.machine = self.library[index].path.clone();
            match loaded
                .load(self.args.options)
                .map_err(|error| -> Box<dyn Error> { Box::new(error) })
                .and_then(|loaded| crate::setup_loaded(&self.args, &loaded, &mut io::stderr()))
            {
                Ok(mut presenter) => {
                    self.prepared.insert(index, loaded);
                    presenter.audio.open(&mut io::stderr());
                    self.started(presenter);
                    self.insert_pending_media(&crate::media_dialog::PendingOwner::MachineFile(
                        self.library[index].path.clone(),
                    ));
                }
                Err(error) => self.show_error(error.to_string()),
            }
        } else {
            self.show_error(
                self.library[index]
                    .warning
                    .clone()
                    .unwrap_or_else(|| "Machine is not ready".into()),
            );
        }
        self.sync_routing();
    }

    fn visible_discovery(&self) -> bool {
        self.readiness
            .as_ref()
            .is_some_and(|job| !job.control.is_cancelled() || job.user_cancelled)
    }

    fn poll_readiness(&mut self, ctx: &egui::Context) {
        let roots = self
            .preferences
            .roots(&self.args.rom_dirs, &self.default_firmware_roots);
        if self.presenter.is_none() && self.discovery_roots != roots {
            self.discovery_roots = roots;
            self.refresh_library();
        }
        if self.selected != self.checked_selection {
            if self.checked_selection.is_some() {
                self.abandon_start();
            }
            if self.message.starts_with("Rescanning ROM folders") {
                self.message = "Ready".into();
            }
            if let Some(job) = &mut self.readiness {
                job.user_cancelled = false;
                job.control.cancel();
                if let Some(entry) = self.library.get_mut(job.index) {
                    entry.checked = false;
                }
            }
            self.checked_selection = self.selected;
            self.discovery_finished = false;
            self.discovery_control = Default::default();
        }
        let ready = self
            .readiness
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(prepared) => Some(Ok(prepared)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = ready {
            if let Some(job) = self.readiness.take() {
                if self.message.starts_with("Rescanning ROM folders") {
                    self.message = "Ready".into();
                }
                let current =
                    job.revision == self.library_revision && self.selected == Some(job.index);
                if job.control.is_cancelled() {
                    if job.user_cancelled {
                        if current {
                            self.library[job.index].checked = true;
                            self.library[job.index].warning =
                                Some("Firmware check cancelled; check again".into());
                            self.discovery_finished = true;
                        }
                    } else {
                        if let Some(entry) = self.library.get_mut(job.index) {
                            entry.checked = false;
                        }
                        self.discovery_finished = false;
                    }
                } else if current {
                    self.discovery_finished = true;
                    match result {
                        Ok(prepared) => self.apply_readiness(job.index, job.revision, prepared),
                        Err(()) => {
                            self.library[job.index].checked = true;
                            self.library[job.index].warning = Some("Firmware check failed".into());
                            self.abandon_start();
                        }
                    }
                }
            }
        }
        if self.readiness.is_none()
            && (self.presenter.is_none()
                || self
                    .dialog
                    .as_ref()
                    .is_some_and(|(title, _)| *title == "ROM folders"))
        {
            if let Some(index) = self.selected.filter(|index| !self.library[*index].checked) {
                let wake = ctx.clone();
                self.library[index].warning = Some(if self.start_request == Some(index) {
                    "Loading machine…".into()
                } else {
                    "Checking firmware…".into()
                });
                self.discovery_control = Default::default();
                self.discovery_finished = false;
                self.readiness = Some(ReadinessJob {
                    index,
                    revision: self.library_revision,
                    user_cancelled: false,
                    control: self.discovery_control.clone(),
                    receiver: shell_model::spawn_library_check_with_compatibility(
                        shell_model::LibraryCheck {
                            path: self.library[index].path.clone(),
                            options: self.args.options,
                            roots: self
                                .preferences
                                .roots(&self.args.rom_dirs, &self.default_firmware_roots),
                            cache: self.firmware_cache.clone(),
                            store_root: self
                                .preferences
                                .store_root
                                .clone()
                                .or_else(|| self.default_store_root.clone()),
                            rescan: std::mem::take(&mut self.rescan),
                            refresh_listing: std::mem::take(&mut self.refresh_listing),
                            control: self.discovery_control.clone(),
                            session: self.discovery_session.clone(),
                            prepared: self.prepared.remove(&index).map(|document| {
                                let mut entry = self.library[index].clone();
                                entry.checked = true;
                                entry.warning = None;
                                (entry, Some(document))
                            }),
                        },
                        move || wake.request_repaint(),
                        self.image_library.compatibility.clone(),
                    ),
                });
            }
        }
        if self.readiness.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn started(&mut self, mut presenter: Presenter) {
        // The shell runs one machine: a PC whose start was under way when a
        // file's machine began is finished, not shown.
        if self.player.is_some() {
            if let Err(error) = presenter.finish_persistence(&mut io::stderr()) {
                eprintln!("{error}");
            }
            return;
        }
        self.reset_keyboard();
        // A report belongs to the session it describes; this one saves its own.
        self.error_report = None;
        self.session = self.session.wrapping_add(1);
        self.events.borrow_mut().session = self.session;
        // Every machine keeps the code trail for Save machine report; it
        // changes nothing the guest does and costs nothing measurable. The
        // report's deliveries start with the first report or with Record
        // exceptions, since the CPU runs measurably slower while it keeps
        // its delivery log.
        presenter.machine_mut().enable_code_trail();
        if self.record_exceptions {
            if let Err(error) = presenter.start_recording() {
                eprintln!("exceptions are not recorded: {error}");
            }
        }
        presenter.window = self.window.clone();
        presenter.started = Some(Instant::now());
        presenter.next_paint = Some(std::time::Duration::ZERO);
        self.presenter = Some(presenter);
        // A draft built before Start lists the stopped machine's catalogue.
        self.insert_dialog = None;
        self.picker = None;
        self.paused = false;
        self.guest_focus = true;
        self.modal_paused = false;
        self.texture = None;
        self.message = format!(
            "Running — click the screen to capture the pointer; {} releases",
            shell_model::shortcut_text("Ctrl+Alt+G", self.macos_shortcuts)
        );
        *self.errors.borrow_mut() = None;
        self.dialog = None;
        self.sync_routing();
    }

    fn refresh_library(&mut self) {
        if let Some(job) = &mut self.readiness {
            job.user_cancelled = false;
            job.control.cancel();
        }
        self.discovery_finished = false;
        self.prepared.clear();
        self.library_revision = self.library_revision.wrapping_add(1);
        // The machine file is read again and its catalogue may have changed,
        // so floppies inserted for Start are taken out rather than kept by a
        // place in a list that no longer holds them.
        if self.presenter.is_none() {
            if self
                .pending_media
                .take()
                .is_some_and(|pending| pending.has_inserts())
            {
                self.message =
                    "Floppies inserted for Start were taken out: the machine was read again".into();
            }
            self.insert_dialog = None;
        }
        self.abandon_start();
        self.discovery_warnings.clear();
        for entry in &mut self.library {
            *entry = LibraryEntry::pending(entry.path.clone());
        }
    }

    fn cancel_firmware(&mut self) {
        let pending = self.readiness.is_some() || self.rescan;
        if let Some(job) = &mut self.readiness {
            job.user_cancelled = true;
            job.control.cancel();
        }
        self.rescan = false;
        self.refresh_listing = false;
        self.abandon_start();
        if pending {
            if let Some(entry) = self.selected.and_then(|index| self.library.get_mut(index)) {
                entry.checked = true;
                entry.warning = Some("Firmware check cancelled; check again".into());
            }
        }
    }

    fn close_firmware_dialog(&mut self) {
        self.dialog = None;
    }

    fn check_firmware_again(&mut self) {
        self.refresh_library();
        self.refresh_listing = true;
        self.rescan = false;
    }

    fn rescan_firmware(&mut self) {
        self.refresh_library();
        self.rescan = true;
        self.message = "Rescanning ROM folders…".into();
    }

    /// The preferences file, or the reason the shell has none.
    fn preferences_file(&self) -> io::Result<&std::path::Path> {
        self.preferences_path.as_deref().ok_or_else(|| {
            io::Error::other(
                self.preferences_path_error
                    .clone()
                    .unwrap_or_else(|| "platform config directory is unavailable".into()),
            )
        })
    }

    fn save_preferences(&mut self) {
        let result = self
            .preferences_file()
            .and_then(|path| self.preferences_draft.save(path));
        match result {
            Ok(()) => {
                self.preferences = self.preferences_draft.clone();
                if self.presenter.is_none() {
                    self.refresh_library();
                }
                self.dialog = None;
                self.message = "ROM folders saved; machine readiness refreshed".into();
            }
            Err(error) => {
                self.dialog = Some(("ROM folders", format!("Cannot save preferences: {error}")));
            }
        }
    }

    /// A CPU stop report saved into [`CPU_STOP_REPORT_FILE`] in the machine's
    /// workspace, or the one already shown when Stop shows again the failure
    /// the same session's run reported. Call it while the session is open.
    fn saved_report(&mut self, text: String) -> ErrorReport {
        match self.error_report.take() {
            Some(shown) if shown.text == text => shown,
            _ => {
                let saved = self.save_cpu_stop_report(&text);
                ErrorReport { text, saved }
            }
        }
    }

    /// Show a session's failure; a CPU refusal's stop report goes in the
    /// dialog, and the status line says where it was saved.
    fn show_machine_failure(&mut self, error: String, report: Option<ErrorReport>) {
        self.show_error(error);
        if let Some(report) = report {
            self.message = format!("{} — {}", self.message, report.saved);
            self.error_report = Some(report);
        }
    }

    /// Write the report beside the session's media, in
    /// [`CPU_STOP_REPORT_DIR`] of the image's workspace or of the machine
    /// file's local folder. Returns the status-line words.
    fn save_cpu_stop_report(&self, text: &str) -> String {
        let saved = self.library_media_dir().and_then(|media| {
            let folder = media
                .parent()
                .ok_or("the session has no workspace folder")?
                .join(CPU_STOP_REPORT_DIR);
            let path = folder.join(CPU_STOP_REPORT_FILE);
            std::fs::create_dir_all(&folder)
                .and_then(|()| std::fs::write(&path, text))
                .map_err(|error| error.to_string())?;
            Ok(path)
        });
        match saved {
            Ok(path) => format!("cpu stop report saved to {}", path.display()),
            Err(error) => format!("cpu stop report not saved: {error}"),
        }
    }

    /// Tools › Diagnostics › Save machine report and its chord: the
    /// running PC's report goes to the next `diag-N.txt`, the exception
    /// recording, when one runs, to its `trace-N.txt`, and the status line
    /// says where. Without a PC nothing happens. See [`crate::diagnostics`].
    fn save_diagnostics(&mut self) {
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        // The first report starts the report's deliveries: it lists none
        // yet, and says so.
        let log_started = !presenter.machine().delivery_log_enabled();
        let log_note = if log_started {
            match presenter.machine_mut().enable_delivery_log() {
                Ok(()) => "desktop  the delivery log starts with this report: the next one lists the deliveries since\n".to_owned(),
                Err(error) => format!("desktop  the delivery log cannot start: {error}\n"),
            }
        } else {
            String::new()
        };
        let folder = self.args.diagnostics.clone();
        let recording = match &folder {
            Ok(folder) => presenter.write_recording(folder),
            Err(error) => presenter.recording().then(|| Err(error.clone())),
        };
        // The machine's status says only whether it can run; the desktop
        // says whether it holds the machine paused.
        let mut text = format!(
            "{}desktop  {}\n{log_note}",
            presenter.machine_mut().diagnostic_report(),
            if self.paused { "paused" } else { "running" }
        );
        text.push_str(&match (&recording, self.record_exceptions) {
            (Some(Ok(path)), _) => format!("desktop  exceptions recorded to {}\n", path.display()),
            (Some(Err(error)), _) => format!("desktop  exceptions not written: {error}\n"),
            (None, true) => format!(
                "desktop  exceptions not recorded: Record exceptions is on, but the recording did not start ({})\n",
                presenter.recording_error().unwrap_or("no reason was given")
            ),
            (None, false) => "desktop  exceptions not recorded (Tools › Diagnostics › Record exceptions is off)\n".to_owned(),
        });
        self.message = match folder
            .and_then(|folder| crate::diagnostics::save(&folder, &text).map_err(|e| e.to_string()))
        {
            Ok(path) => format!("Machine report saved to {}", path.display()),
            Err(error) => format!("Machine report not saved: {error}"),
        };
        match recording {
            Some(Ok(path)) => self
                .message
                .push_str(&format!("; exceptions to {}", path.display())),
            Some(Err(error)) => self
                .message
                .push_str(&format!("; exceptions not written: {error}")),
            None => {}
        }
    }

    /// Tools › Diagnostics › Record exceptions: on, the running PC and every
    /// later one record their exceptions; off, the running PC's recording
    /// is written to its file and ends.
    fn set_record_exceptions(&mut self, on: bool) {
        self.record_exceptions = on;
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        if on {
            if let Err(error) = presenter.start_recording() {
                self.message = format!("Exceptions are not recorded: {error}");
            } else {
                self.message = format!(
                    "Recording exceptions: the last {} entries",
                    unemu_pc_core::RECORDED_DELIVERIES
                );
            }
        } else if let Some(message) = self.finish_recording() {
            self.message = message;
        }
    }

    /// Write the running PC's exception recording to its file and end it;
    /// the status line's words for it, `None` when nothing recorded.
    fn finish_recording(&mut self) -> Option<String> {
        recording_finished(&self.args.diagnostics, self.presenter.as_mut()?)
    }

    /// Tools › Diagnostics › Open diagnostics folder.
    fn open_diagnostics_folder(&mut self) {
        let opened = self.args.diagnostics.clone().and_then(|folder| {
            crate::diagnostics::reveal(&folder)
                .map(|()| folder)
                .map_err(|error| error.to_string())
        });
        self.message = match opened {
            Ok(folder) => format!("Diagnostics folder: {}", folder.display()),
            Err(error) => format!("Cannot open the diagnostics folder: {error}"),
        };
    }

    fn show_error(&mut self, error: String) {
        self.error_report = None;
        self.insert_dialog = None;
        self.blank_disk = None;
        self.stop_prompt = None;
        self.opened = None;
        self.release();
        if let Some(presenter) = &mut self.presenter {
            presenter.running = false;
            presenter.host_failed = true;
        }
        *self.errors.borrow_mut() = Some(error.clone());
        self.message = error.clone();
        self.dialog = Some(("Machine error", error));
    }

    /// The session ends only once a path is confirmed, and only after the
    /// stop prompt; closing the dialog leaves a running machine untouched.
    fn open_machine(&mut self) {
        if self.needs_stop_prompt() {
            self.dialog = None;
            self.ask_before_stopping(StopPrompt::OpenMachine);
        } else {
            self.open_machine_path();
        }
    }

    fn open_machine_path(&mut self) {
        if !self.stop() {
            return;
        }
        let path = std::path::PathBuf::from(self.open_path.trim());
        let entry = LibraryEntry::pending(path);
        self.library.push(entry);
        self.selected = Some(self.library.len() - 1);
        self.dialog = None;
    }

    fn stop(&mut self) -> bool {
        self.insert_dialog = None;
        self.picker = None;
        self.blank_disk = None;
        self.stop_prompt = None;
        self.release();
        self.session = self.session.wrapping_add(1);
        self.events.borrow_mut().session = self.session;
        self.start_request = None;
        self.drop_opening();
        self.opened = None;
        if let Some(player) = self.player.take() {
            return self.stop_player(player);
        }
        let mut failure = None;
        let had_session = self.presenter.is_some();
        let report = self
            .presenter
            .as_ref()
            .filter(|presenter| presenter.error.is_some())
            .and_then(|presenter| presenter.machine().cpu_stop_report())
            .map(ToString::to_string);
        // The workspace is found while the session is still open.
        let report = report.map(|text| self.saved_report(text));
        let mut catalogue = Vec::new();
        let mut recorded = None;
        if let Some(mut presenter) = self.presenter.take() {
            // The drives' catalogues as they are now, with the media changes
            // queued for the next run boundary in, which a library machine
            // keeps for its next Start.
            let inserted = [0u8, 1].map(|unit| {
                let now = presenter.inserted_floppy(usize::from(unit));
                let count = presenter
                    .assembled_ref()
                    .floppy_names
                    .get(&(assembly::DeviceId::Fdc, unit))
                    .map_or(0, Vec::len);
                crate::kept_catalogue::after_queued(unit, now, count, &presenter.media_events)
            });
            catalogue = crate::kept_catalogue::stopped(&presenter.media_slots, inserted);
            if (presenter.running || presenter.halted) && presenter.error.is_none() {
                if let Err(error) = presenter.finish_persistence(&mut io::stderr()) {
                    failure = Some(error.to_string());
                }
            }
            if let Some(error) = &presenter.error {
                failure.get_or_insert_with(|| error.to_string());
            }
            presenter.write_reset_summary(&mut io::stderr());
            recorded = recording_finished(&self.args.diagnostics, &mut presenter);
        }
        let retain_workspace = std::mem::take(&mut self.retain_workspace_on_stop);
        if had_session {
            self.image_library.keep_catalogue(catalogue);
            self.image_library
                .finish_with_failure(retain_workspace, failure.is_some());
        }
        self.texture = None;
        self.paused = false;
        self.modal_paused = false;
        self.message = match recorded {
            Some(recorded) => format!("Ready — {recorded}"),
            None => "Ready".into(),
        };
        if had_session {
            self.refresh_library();
        }
        self.sync_routing();
        if let Some(error) = failure {
            self.show_machine_failure(error, report);
            false
        } else {
            if had_session {
                *self.errors.borrow_mut() = None;
                self.dialog = None;
            }
            true
        }
    }

    /// Finish the machine started from a file and show the library, as
    /// [`Shell::stop`] does for the PC: a failure to finish, or the guest's
    /// failure that ended the run, is shown and makes the stop fail.
    fn stop_player(&mut self, mut player: player_ui::Player) -> bool {
        let elapsed = player.elapsed();
        let failure = match player.command(player_ui::Command::Stop, elapsed, &mut io::stderr()) {
            Some(player_ui::Request::Stopped { failure }) => failure,
            _ => None,
        };
        drop(player);
        self.texture = None;
        self.paused = false;
        self.modal_paused = false;
        self.message = "Ready".into();
        self.sync_routing();
        match failure {
            Some(error) => {
                self.show_error(error);
                false
            }
            None => {
                *self.errors.borrow_mut() = None;
                self.dialog = None;
                true
            }
        }
    }

    /// `--open FILE [--core ID]`: the file is prepared as File › Open…
    /// prepares it, once the window shows.
    fn open_on_start(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.args.open.clone() {
            let core = self.args.core.clone();
            self.open_file(path, core, ctx);
        }
    }

    /// Prepare `path` on a worker thread: hash it, ask the cores (only
    /// `core` when named) and check what the answering core wrote. The
    /// answer wakes the window through `ctx`. A file asked for later
    /// replaces one still being prepared.
    fn open_file(&mut self, path: std::path::PathBuf, core: Option<String>, ctx: &egui::Context) {
        let dirs = match &self.host_dirs {
            Ok(dirs) => dirs.clone(),
            Err(reason) => {
                self.open_refusal = Some(player_ui::MessageBox::refusal("Open", reason.clone()));
                return;
            }
        };
        let cores = self.cores.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let spawned = std::thread::Builder::new()
            .name("open-file".into())
            .spawn(move || {
                let _ = sender.send(core_launch::prepare(&path, &cores, core.as_deref(), &dirs));
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => {
                // A file replaced while it is prepared keeps the first
                // message to come back to.
                let before = match self.opening.take() {
                    Some((_, before)) => before,
                    None => std::mem::take(&mut self.message),
                };
                self.message = format!("Opening {name}…");
                self.opening = Some((receiver, before));
            }
            Err(error) => {
                self.open_refusal = Some(player_ui::MessageBox::refusal(
                    "Open",
                    format!("The file could not be opened: {error}"),
                ));
            }
        }
    }

    /// Take the prepared file, once the worker has it.
    fn poll_opening(&mut self) {
        let Some((receiver, _)) = &self.opening else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
        };
        self.drop_opening();
        if let Some(result) = result {
            self.opened_file(result);
        }
    }

    /// Forget the file being prepared, and give the status line back its
    /// message.
    fn drop_opening(&mut self) {
        if let Some((_, before)) = self.opening.take() {
            self.message = before;
        }
    }

    /// A file that cannot start shows its refusal and leaves a running
    /// machine alone; one that can asks to stop a running machine first,
    /// with the stop prompt, then starts.
    fn opened_file(&mut self, result: OpenResult) {
        // The question already asked, or the exit under way, decides what
        // happens next; a file prepared meanwhile is dropped, not slipped
        // under the answer.
        if self.stop_prompt.is_some() || self.exit_requested {
            return;
        }
        let prepared = match result.and_then(core_launch::Outcome::into_ready) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.open_refusal = Some(player_ui::MessageBox::refusal("Open", error.to_string()));
                return;
            }
        };
        if self.ask_before_stopping(StopPrompt::OpenFile) {
            self.opened = Some(prepared);
            return;
        }
        if (self.presenter.is_some() || self.player.is_some()) && !self.stop() {
            return;
        }
        self.start_player(prepared);
    }

    /// Build the prepared file's machine on this thread, which drives it,
    /// with the pad keys of the preferences.
    fn start_player(&mut self, prepared: core_launch::Prepared) {
        let bindings = self.preferences.pad_bindings();
        let mut player = match player_ui::Player::start(prepared, bindings, &mut io::stderr()) {
            Ok(player) => player,
            Err(error) => {
                self.open_refusal = Some(player_ui::MessageBox::refusal("Open", error.to_string()));
                return;
            }
        };
        // The tests play no sound, as for the PC's machines.
        if !cfg!(test) {
            player.open_audio(&mut io::stderr());
        }
        player.set_window(self.window.clone());
        player.set_look(self.classic, self.macos_shortcuts);
        // A machine file still loading for Start never starts beside this
        // one; an image's start in flight is dropped when it arrives.
        self.abandon_start();
        self.reset_keyboard();
        self.error_report = None;
        self.session = self.session.wrapping_add(1);
        self.events.borrow_mut().session = self.session;
        self.insert_dialog = None;
        self.picker = None;
        self.player = Some(player);
        self.paused = false;
        self.guest_focus = true;
        self.modal_paused = false;
        self.texture = None;
        *self.errors.borrow_mut() = None;
        self.dialog = None;
        self.sync_routing();
    }

    /// Carry out a command of the player's own menus, or one of the shell's
    /// hotkeys and toolbar that meets the running player.
    fn player_command(&mut self, command: player_ui::Command, ctx: &egui::Context) {
        match command {
            player_ui::Command::Stop => {
                self.stop();
            }
            player_ui::Command::Fullscreen => self.command(Command::Fullscreen, ctx),
            command => {
                let Some(player) = &mut self.player else {
                    return;
                };
                let elapsed = player.elapsed();
                let request = player.command(command, elapsed, &mut io::stderr());
                self.player_request(request, ctx);
            }
        }
        self.sync_routing();
    }

    /// What the application does after a command or a dialog of the player.
    fn player_request(&mut self, request: Option<player_ui::Request>, ctx: &egui::Context) {
        match request {
            None => {}
            Some(player_ui::Request::Stopped { .. }) => {
                self.stop();
            }
            Some(player_ui::Request::Fullscreen) => self.command(Command::Fullscreen, ctx),
            Some(player_ui::Request::PadBindings) => {
                let Some(player) = &self.player else {
                    return;
                };
                self.preferences.set_pad_bindings(player.pad_bindings());
                self.preferences_draft
                    .set_pad_bindings(player.pad_bindings());
                let saved = self
                    .preferences_file()
                    .and_then(|path| self.preferences.save(path));
                if let Err(error) = saved {
                    self.message = format!("Cannot save preferences: {error}");
                }
            }
        }
    }

    fn stop_for_image_action(&mut self, action: &crate::library_view::Request) -> bool {
        use crate::library_view::Request;
        let target = match action {
            Request::Publish(_, name, _) | Request::Discard(name) => name,
            // Save as new image… of the running machine stops it first.
            Request::Machine(crate::library_worker::MachineJob::Commit { id, .. }) => id,
            _ => return true,
        };
        let matches = self
            .image_library
            .active
            .as_ref()
            .is_none_or(|(_, name, _)| name == target);
        self.retain_workspace_on_stop = matches && self.presenter.is_some();
        let stopped = self.stop();
        if !stopped || !matches {
            self.image_library.cancel_operation();
            return false;
        }
        true
    }

    /// Exit, Command+Q/W and the window's close button all ask first while a
    /// machine runs; the close itself waits for the answer.
    fn request_exit(&mut self, ctx: &egui::Context) {
        // A transfer is not stopped behind the user's back: quitting asks first.
        if let Some(kind) = self.image_library.running_transfer() {
            self.transfer_prompt = Some(kind);
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        self.close_dialogs_for_exit(ctx);
        if self.ask_before_stopping(StopPrompt::Exit) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else {
            self.exit(ctx);
        }
    }

    fn exit(&mut self, ctx: &egui::Context) {
        self.exit_requested = true;
        self.drop_opening();
        let stopped = self.stop();
        if !stopped && self.events.borrow().native.system_quit_pending() {
            // Logout or shutdown would take the error dialog away unseen, so
            // that quit is cancelled and the dialog stays: the session's
            // changes were not finished.
            self.exit_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        if self.image_library.needs_decision() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        } else {
            self.image_library.shutdown();
            self.close_window(ctx);
        }
    }

    /// Stop and persistence have finished. A system quit waiting for this is
    /// told to terminate, and AppKit then closes the window itself.
    fn close_window(&mut self, ctx: &egui::Context) {
        self.answer_system_quit(crate::app_menu::ExitProgress::Closing, ctx);
        if !self.events.borrow().native.terminating() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn exit_progress(&self) -> crate::app_menu::ExitProgress {
        use crate::app_menu::ExitProgress;
        if self.stop_prompt == Some(StopPrompt::Exit) || self.transfer_prompt.is_some() {
            ExitProgress::Asking
        } else if self.exit_requested {
            ExitProgress::Waiting
        } else {
            ExitProgress::Idle
        }
    }

    /// Dock › Quit, logout and shutdown wait while the shell's Exit runs: the
    /// stop prompt comes forward, Stop machine answers terminate once
    /// persistence has finished, and Cancel answers keep running.
    fn answer_system_quit(&mut self, exit: crate::app_menu::ExitProgress, ctx: &egui::Context) {
        use crate::app_menu::QuitStep;
        let step = self.events.borrow_mut().native.settle(exit);
        match step {
            QuitStep::Wait => {}
            QuitStep::ShowPrompt => {
                #[cfg(target_os = "macos")]
                crate::macos_menu::unhide();
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            QuitStep::Reply(terminate) => {
                if terminate {
                    // AppKit ends the process without returning from the
                    // event loop, so report a failed stop here.
                    if let Some(error) = self.errors.borrow().as_deref() {
                        eprintln!("{error}");
                    }
                }
                #[cfg(target_os = "macos")]
                crate::macos_menu::reply_to_terminate(terminate);
            }
        }
    }

    /// Quitting closes or cancels whatever dialog or menu is open. The stop
    /// prompt and the Gaming-mode keep-or-discard question stay, because they
    /// decide how the exit proceeds.
    fn close_dialogs_for_exit(&mut self, ctx: &egui::Context) {
        egui::Popup::close_all(ctx);
        if self
            .dialog
            .as_ref()
            .is_some_and(|(title, _)| *title == "ROM folders")
        {
            self.close_firmware_dialog();
        }
        self.dialog = None;
        self.insert_dialog = None;
        self.picker = None;
        self.blank_disk = None;
        self.settings = None;
        self.image_library.close_dialogs();
    }

    /// The window's close button arrives as a viewport close request.
    fn window_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested()) {
            self.request_exit(ctx);
        } else if self.exit_requested
            && !self.image_library.needs_decision()
            && !self.image_library.busy
            && !self.has_dialog()
        {
            self.image_library.shutdown();
            self.close_window(ctx);
        }
    }

    fn session_name(&self) -> Option<&str> {
        if let Some(player) = &self.player {
            return Some(player.prepared().title());
        }
        if let Some((_, name)) = &self.image_library.active_machine {
            return Some(name);
        }
        self.image_library
            .active
            .as_ref()
            .map(|(row, _, _)| row.name())
            .or_else(|| {
                self.selected
                    .and_then(|index| self.library.get(index))
                    .map(|entry| entry.name.as_str())
            })
    }

    /// Guest memory only matters while a program runs in it. A halted guest
    /// waits for a reset with nothing running, a failed session has nothing
    /// left to lose, and a Gaming-mode session already asks to keep or
    /// discard its changes once stopped.
    fn needs_stop_prompt(&self) -> bool {
        let disposable = self
            .image_library
            .active
            .as_ref()
            .is_some_and(|(_, _, kind)| kind.disposable());
        let player = self.player.as_ref().is_some_and(player_ui::Player::running);
        player
            || (!disposable
                && self
                    .presenter
                    .as_ref()
                    .is_some_and(|presenter| presenter.running && presenter.error.is_none()))
    }

    /// Pause the running machine and ask before `action` ends the session.
    /// Returns false when nothing needs asking and the caller may proceed.
    fn ask_before_stopping(&mut self, action: StopPrompt) -> bool {
        if !self.needs_stop_prompt() {
            return false;
        }
        if self.stop_prompt.is_none() {
            self.stop_prompt_input = (
                self.guest_focus,
                self.presenter
                    .as_ref()
                    .is_some_and(|presenter| presenter.mouse.captured())
                    || self
                        .player
                        .as_ref()
                        .is_some_and(player_ui::Player::pointer_captured),
            );
        }
        self.release();
        // Pause at once rather than at the next repaint, so no guest time
        // passes between the request and the question.
        let elapsed = self.elapsed();
        if let Some(presenter) = &mut self.presenter {
            presenter.occlusion_changed(true, elapsed);
        }
        self.modal_paused = true;
        self.stop_prompt = Some(action);
        true
    }

    /// Stop machine performs the held action. Cancel only closes the question:
    /// the next tick resumes the machine where it paused, and the guest gets
    /// back the keyboard focus and pointer capture it had.
    fn answer_stop_prompt(&mut self, stop: bool, ctx: &egui::Context) {
        let Some(action) = self.stop_prompt.take() else {
            return;
        };
        let opened = self.opened.take();
        if !stop {
            self.restore_input = Some(self.stop_prompt_input);
            return;
        }
        match action {
            StopPrompt::Library => {
                self.stop();
            }
            StopPrompt::OpenMachine => self.open_machine_path(),
            StopPrompt::OpenFile => {
                if let Some(prepared) = opened {
                    if self.stop() {
                        self.start_player(prepared);
                    }
                }
            }
            StopPrompt::Exit => self.exit(ctx),
        }
    }

    /// Cancel the running transfer and quit, or keep it running. Cancelling
    /// drops the dialog, whose transfer stops at its next check.
    fn answer_transfer_prompt(&mut self, quit: bool, ctx: &egui::Context) {
        if self.transfer_prompt.take().is_none() || !quit {
            return;
        }
        self.image_library.close_dialogs();
        self.request_exit(ctx);
    }

    /// The question before quitting while a push or pull runs; `escape` is an
    /// Escape already taken for it, so the transfer dialog beneath never sees
    /// the key and cancels.
    fn transfer_prompt_modal(&self, ctx: &egui::Context, escape: bool) -> Option<bool> {
        let kind = self.transfer_prompt?;
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("quit-during-transfer")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            if self.classic {
                gradient_title(ui, "unemu");
            } else {
                ui.heading("unemu");
            }
            ui.add(egui::Label::new(format!("A {kind} is running. Cancel it and quit?")).wrap());
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(format!("Cancel {kind} and quit")).clicked() {
                    answer = Some(true);
                }
                if ui.button(format!("Keep the {kind} running")).clicked() {
                    answer = Some(false);
                }
            });
            if escape {
                answer = Some(false);
            }
        });
        answer
    }

    /// `escape` reports an Escape already taken for this prompt, the top-most
    /// dialog, before the dialogs beneath it could see the same key.
    fn stop_prompt_modal(&self, ctx: &egui::Context, escape: bool) -> Option<bool> {
        self.stop_prompt?;
        let question = format!(
            "unemu is running {}. Stop it and continue?",
            self.session_name().unwrap_or("a machine")
        );
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("stop-running-machine")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            if self.classic {
                gradient_title(ui, "unemu");
            } else {
                ui.heading("unemu");
            }
            ui.add(egui::Label::new(question).wrap());
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Stop machine").clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
            if escape || ui.input_mut(escape_pressed) {
                answer = Some(false);
            }
        });
        answer
    }

    /// Hand input back after a cancelled stop prompt, on the first pass with no
    /// menu or dialog: the pass that closed the prompt still releases input.
    fn resume_input(&mut self, blocked: bool) {
        if blocked {
            return;
        }
        let Some((focus, captured)) = self.restore_input.take() else {
            return;
        };
        if self.presenter.is_none() && self.player.is_none() {
            return;
        }
        if focus {
            self.focus_guest();
        }
        if captured {
            if let Some(presenter) = &mut self.presenter {
                presenter.recapture_pointer();
            }
            if let Some(player) = &mut self.player {
                player.recapture_pointer();
            }
            self.sync_routing();
        }
    }

    fn open_machine_dialog(&mut self) {
        self.dialog = Some((
            "Open machine file",
            "Open a machine description in the library.".into(),
        ));
    }

    fn command(&mut self, command: Command, ctx: &egui::Context) {
        self.sync_pending_media();
        if !self.command_allowed(command) {
            return;
        }
        if let Some(command) = self.for_player(command) {
            self.player_command(command, ctx);
            return;
        }
        let elapsed = self.elapsed();
        use Command::*;
        match command {
            Open => { self.release(); self.ask_file(FileFor::Open); },
            Speed(speed) => if let Some(presenter) = &mut self.presenter { presenter.select_speed(speed, elapsed); },
            FastForward => if let Some(presenter) = &mut self.presenter { presenter.hold_fast_forward(true, elapsed); },
            Start => self.start(),
            GamingMode => { if self.image_library.selected.is_some() { self.image_library.launch(crate::library_worker::WorkspaceKind::Disposable); } },
            Layers => { self.release(); self.image_library.open_layers();
                if self.image_library.selected.is_none() {
                    if let (Some(entry),Some(dialog))=(self.selected.and_then(|i|self.library.get(i)),&mut self.image_library.layers){dialog.set_machine_name(&entry.name);}
                }
            },
            ImportImage => { self.release(); if let Some(entry) = self.selected.and_then(|i| self.library.get(i)) { self.image_library.begin_import(&entry.path); } },
            DeleteTag => { self.release(); self.image_library.begin_delete(); },
            ImportFirmware => {
                self.release();
                self.ask_file(FileFor::RomImport);
            },
            StoreFolder => { self.release(); self.store_path_input = self.image_library.root.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(); self.dialog = Some(("Image store folder", "Local store directory".into())); },
            Pause => { self.release_pointer(); if let Some(presenter) = &mut self.presenter { presenter.resync(elapsed); } self.paused = !self.paused; self.message = if self.paused { "Paused" } else { "Running" }.into(); },
            Stop => { self.stop(); },
            Library => if !self.ask_before_stopping(StopPrompt::Library) { self.stop(); },
            PowerCycle => {
                let image_session = self.image_library.active.clone();
                self.retain_workspace_on_stop = image_session.is_some();
                if self.stop() {
                    if let Some((row, name, kind)) = image_session { self.image_library.resume(row, name, kind); }
                    else { self.start(); }
                }
            },
            Reset => { self.guest_held.clear();
                if self.presenter.as_ref().is_some_and(|presenter| presenter.awaits_reset()) {
                    self.reset_halted();
                } else if let Some(presenter) = &mut self.presenter {
                    presenter.machine_mut().ctrl_alt_del();
                }
            },
            // One Insert… per drive opens the media picker; it replaces the
            // catalogue dialog and From library….
            Insert(unit) => { self.release(); self.open_picker(unit, None, None); },
            // Create from files… opens the picker straight in its Create view.
            CreateFromFiles(unit) => {
                self.release();
                self.open_picker(unit, None, None);
                if let Some(picker) = &mut self.picker { picker.open_create(); }
            },
            InsertFile(unit) => if let Some(slot) = self.floppy_slot(unit) {
                self.release();
                self.insert_dialog = Some(crate::media_dialog::InsertDialog::new(unit, slot));
            },
            InsertSet(unit) => {
                let set = self.image_library.selected_set();
                self.release();
                self.open_picker(unit, None, set);
            },
            ImportMedia => { self.release(); self.image_library.begin_media_import(); },
            EditFirmware => { self.release(); self.image_library.begin_firmware_edit(false); },
            WriteMode(unit) => { self.command(InsertFile(unit), ctx); self.select_inserted(unit); },
            CreateBlank => {
                self.release();
                let entry = self.selected.and_then(|index| self.library.get(index));
                self.blank_disk = Some(crate::media_dialog::BlankDisk::for_machine(entry.map_or(self.args.machine.as_path(), |entry| entry.path.as_path()), entry.and_then(|entry| entry.warning.as_deref())));
            },
            CatalogueDisk(unit, index) => self.choose_catalogue_disk(unit, index),
            CatalogueAll(unit) => { self.release(); self.open_catalogue_picker(unit); },
            Next(unit) | Eject(unit) => if let Some(presenter) = &mut self.presenter {
                presenter.media_events.push((SlotId(unit), if matches!(command, Next(_)) { MediaAction::Next } else { MediaAction::Eject }));
            } else if let (Eject(_), Some(row)) = (command, self.image_library.selected_machine_row().cloned()) {
                // Take out again: the stopped machine's pending disk goes.
                let drive = crate::media_picker::drive_name(unit);
                self.image_library.set_pending(row.id(), &drive, None);
                self.message = format!("Took the pending disk out of {drive} of {}", row.name());
            } else if let (Eject(_), Some(pending)) = (command, &mut self.pending_media) {
                pending.eject(unit);
                self.message = format!("{}: ejected before Start", if unit == 0 { "A" } else { "B" });
            },
            Mute => if let Some(presenter) = &mut self.presenter { presenter.audio.toggle_mute(); },
            Release => self.release_pointer(),
            Fullscreen => { self.fullscreen = !self.fullscreen; ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen)); },
            Minimize => { self.release(); ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)); },
            Exit => self.request_exit(ctx),
            Shortcuts => self.dialog = Some(("Keyboard shortcuts", shortcuts_help(self.macos_shortcuts))),
            About => self.dialog = Some(("About unemu", format!("unemu {}\nemulator of period IBM-compatible PCs\n{}\nx86jit · egui / eframe · cpal\n\n{}\n\n{}\n\n{}\n\n{}", env!("CARGO_PKG_VERSION"), platforms::licence_notice(), unemu_pc_core::FONT_NOTICE, ui_kit::EGUI_FONT_NOTICE, ui_kit::SYMBOL_FONT_NOTICE, devices::OPL3_NOTICE))),
            Placeholder("Open machine file") => self.open_machine_dialog(),
            Placeholder("New machine") => { self.release(); self.image_library.begin_new_machine(); },
            Placeholder(title) => self.dialog = Some((title, "not yet available".into())),
            FirmwareFolders => { self.preferences_draft = self.preferences.clone(); self.rom_folder_input.clear(); self.dialog = Some(("ROM folders", "Firmware is located by SHA-256. Files are read in place; nothing is copied.".into())); },
            Settings => if self.image_library.selected.is_some() { self.release(); self.image_library.open_settings(); } else { self.open_settings() },
            SaveImage => { self.release(); self.image_library.begin_save_image(); },
            Preferences => self.open_preferences(),
            SaveState | LoadState => {},
            Push => { self.release(); let accounts = self.registry_accounts(); self.image_library.begin_push(accounts); },
            PushHere => { self.release(); if let Some(place) = self.image_library.push_place() { self.image_library.begin_push_here(place); } },
            Pull => { self.release(); self.image_library.begin_remote_pull(crate::remote_pull::Mode::Pull); },
            PullReference => { self.release(); let accounts = self.registry_accounts(); self.image_library.begin_pull(accounts); },
            GetTemplates => { self.release(); let accounts = self.registry_accounts(); self.image_library.show_templates(accounts, &mut self.selected); },
            RefreshRemote => self.image_library.refresh_remote(),
            Rename => { self.release(); self.image_library.begin_rename(); },
            Duplicate => { self.release(); self.image_library.begin_duplicate(); },
            DeleteMachine => { self.release(); self.image_library.begin_delete_machine(); },
            DeleteSelection => {
                self.release();
                if self.image_library.machine.is_some() { self.image_library.begin_delete_machine(); } else { self.image_library.begin_delete(); }
            },
            ResetMachine => { self.release(); self.image_library.begin_reset(); },
            ShowSource => self.image_library.show_source_image(),
            NewFromImage => { self.release(); self.image_library.begin_new_from_image(); },
            ImageStart => self.image_library.start_selected_image(),
            ImageGaming => self.image_library.image_gaming(),
            ImageSettings => { self.release(); self.image_library.open_image_settings(); },
            ImageLayers => { self.release(); self.image_library.open_image_layers(); },
        }
        self.sync_routing();
    }

    /// The player's command for a shell command that meets the running
    /// machine, while a machine started from a file runs: from the toolbar,
    /// the hotkeys and the application menu. The media keys work the first
    /// two removable slots.
    fn for_player(&self, command: Command) -> Option<player_ui::Command> {
        use player_ui::Command as Player;
        let player = self.player.as_ref()?;
        let slot = |unit: u8| player.offers().media.get(usize::from(unit)).copied();
        Some(match command {
            Command::Pause => Player::Pause,
            Command::Reset => Player::Reset,
            Command::PowerCycle => Player::PowerCycle,
            Command::Stop => Player::Stop,
            Command::SaveState => Player::SaveState,
            Command::LoadState => Player::LoadState,
            Command::Mute => Player::Mute,
            Command::Speed(speed) => Player::Speed(speed),
            Command::FastForward => Player::FastForward(true),
            Command::Release => Player::Release,
            Command::Next(unit) => Player::Next(slot(unit)?),
            Command::Eject(unit) => Player::Eject(slot(unit)?),
            _ => return None,
        })
    }

    fn item(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        shortcut: &str,
        command: Command,
        commands: &mut Vec<Command>,
    ) {
        self.menu_item(ui, None, label, shortcut, command, commands);
    }

    /// A menu item with the main-window mock-up's mark column: the glyph
    /// (▶ Ⅱ ■ ⏏) or "" for an empty column, so the menu's labels line up.
    fn marked_item(
        &self,
        ui: &mut egui::Ui,
        mark: &'static str,
        label: &str,
        shortcut: &str,
        command: Command,
        commands: &mut Vec<Command>,
    ) {
        self.menu_item(ui, Some(mark), label, shortcut, command, commands);
    }

    /// Drive `unit`'s Catalogue item and submenu, growing `upward` or down;
    /// the disk chosen there, or All N disks…, becomes its command.
    fn catalogue_item(
        &self,
        ui: &mut egui::Ui,
        unit: u8,
        inserted: [Option<usize>; 2],
        upward: bool,
        commands: &mut Vec<Command>,
    ) {
        use crate::catalogue_menu::Chosen;
        let catalogue = self.catalogue(unit, inserted);
        match crate::catalogue_menu::item(ui, catalogue.as_ref(), upward) {
            Some(Chosen::Disk(index)) => commands.push(Command::CatalogueDisk(unit, index)),
            Some(Chosen::All) => commands.push(Command::CatalogueAll(unit)),
            None => {}
        }
    }

    fn menu_item(
        &self,
        ui: &mut egui::Ui,
        mark: Option<&'static str>,
        label: &str,
        shortcut: &str,
        command: Command,
        commands: &mut Vec<Command>,
    ) {
        let button = match mark {
            Some(mark) => egui::Button::new((ui_kit::shell_icons::mark(mark), label)),
            None => egui::Button::new(label),
        };
        let response = ui.add_enabled(
            self.command_allowed(command),
            button.shortcut_text(shell_model::shortcut_text(shortcut, self.macos_shortcuts)),
        );
        // A disabled item says in one short phrase why.
        let response = match self.disabled_reason(command) {
            Some(reason) => response.on_disabled_hover_text(reason),
            None => response,
        };
        if response.clicked() {
            commands.push(command);
            ui.close();
        }
    }

    /// Why a library command is unavailable now, in one short phrase.
    fn disabled_reason(&self, command: Command) -> Option<&'static str> {
        use Command::*;
        if self.image_library.several_selected() && single_item(command) {
            return Some("Several items are selected: select one");
        }
        let machine = self.image_library.machine.is_some();
        // Delete tag… also takes the selected firmware package.
        let image = self.image_library.selected.is_some()
            || (command == DeleteTag && self.image_library.can_delete_package());
        let running = self.presenter.is_some() || self.player.is_some();
        Some(match command {
            Open if !self.opens_files() => "No core in this release opens files",
            Rename | Duplicate | DeleteMachine | ResetMachine | ShowSource if !machine => {
                "Select a machine"
            }
            Duplicate | DeleteMachine | ResetMachine | ShowSource if running => {
                "Stop the machine first"
            }
            DeleteTag if machine => "Select an image: machines are deleted with Delete machine…",
            DeleteTag | ImageStart | ImageGaming | NewFromImage | ImageSettings | ImageLayers
                if !image =>
            {
                "Select an image or a machine"
            }
            ImageStart | ImageGaming | NewFromImage | ImageSettings | DeleteTag if running => {
                "Stop the machine first"
            }
            Start | GamingMode if running => "A machine runs already",
            Start | GamingMode if !image && self.selected.is_none() => "Select a machine",
            Pull if !self.image_library.can_pull_remote() => "Select a tag in Remotes to pull",
            Push if !self.image_library.can_push_to() => {
                "Select images, installation media or firmware packages in the library"
            }
            PushHere if self.image_library.push_place().is_none() => {
                "Select a registry or namespace in Remotes"
            }
            PushHere if !self.image_library.can_push_here() => {
                "The registry does not answer; Push here… waits until Try again reaches it"
            }
            _ => return None,
        })
    }

    fn menus(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        use Command::*;
        let mut diagnostics = None;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                for (label, shortcut, command) in [
                    ("New machine…", "Ctrl+N", Placeholder("New machine")),
                    ("Get templates…", "", GetTemplates),
                    ("Open…", "Ctrl+O", Open),
                    ("Open machine file…", "", Placeholder("Open machine file")),
                    ("Import image…", "", ImportImage),
                    ("Import installation media…", "", ImportMedia),
                    ("Import ROM folder…", "", ImportFirmware),
                    ("Edit metadata…", "", EditFirmware),
                ] {
                    self.item(ui, label, shortcut, command, commands);
                }
                ui.separator();
                self.item(ui, "Machine library", "", Library, commands);
                self.item(
                    ui,
                    "Exit",
                    if self.macos_shortcuts { "⌘Q" } else { "" },
                    Exit,
                    commands,
                );
            });
            ui.menu_button("Edit", |ui| {
                self.item(ui, "Rename…", "F2", Rename, commands);
                self.item(ui, "Duplicate…", "Ctrl+D", Duplicate, commands);
                self.item(ui, "Delete machine…", "Delete", DeleteMachine, commands);
                self.item(ui, "Delete tag…", "Delete", DeleteTag, commands);
            });
            ui.menu_button("Image", |ui| {
                // The Image menu acts on the selected image, or on the
                // selected machine's source image, and names it first.
                let target = match (&self.image_library.selected, &self.image_library.machine) {
                    (Some(row), Some(_)) => format!(
                        "For the image: {} — source of {}",
                        row.reference,
                        self.image_library
                            .selected_machine_row()
                            .map_or("the machine", |m| m.name())
                    ),
                    (Some(row), None) => format!("For the image: {}", row.reference),
                    (None, _) => "Select an image or a machine".into(),
                };
                ui.add_enabled(false, egui::Label::new(target));
                self.item(ui, "Start…", "Enter", ImageStart, commands);
                self.item(
                    ui,
                    "New machine from image…",
                    "Ctrl+Shift+N",
                    NewFromImage,
                    commands,
                );
                self.item(ui, "Gaming mode", "", ImageGaming, commands);
                self.item(
                    ui,
                    "Settings… (new image revision)",
                    "",
                    ImageSettings,
                    commands,
                );
                self.item(ui, "Layers…", "", ImageLayers, commands);
                ui.separator();
                self.item(ui, "Pull…", "Enter", Pull, commands);
                self.item(ui, "Pull by reference…", "", PullReference, commands);
                ui.separator();
                self.item(ui, "Push to…", "", Push, commands);
                self.item(ui, "Push here…", "", PushHere, commands);
            });
            // A machine started from a file has the player's Machine, Media
            // and View menus where the PC's stand.
            if let Some(player) = &self.player {
                player.menus(
                    ui,
                    player_ui::ViewChrome {
                        toolbar: &mut self.toolbar,
                        status_bar: &mut self.statusbar,
                        classic: &mut self.classic,
                    },
                    &mut self.player_commands,
                );
            } else {
                self.pc_menus(ui, commands);
            }
            ui.menu_button("Tools", |ui| {
                self.item(ui, "Image store folder…", "", StoreFolder, commands);
                self.item(ui, "Import ROM folder…", "", ImportFirmware, commands);
                for title in [
                    "Media catalogue",
                    "Locate firmware",
                    "Create blank disk",
                    "Preferences",
                ] {
                    self.item(
                        ui,
                        &format!("{title}…"),
                        "",
                        match title {
                            "Locate firmware" => FirmwareFolders,
                            "Preferences" => Preferences,
                            "Media catalogue" => Insert(0),
                            "Create blank disk" => CreateBlank,
                            _ => Placeholder(title),
                        },
                        commands,
                    );
                }
                ui.separator();
                ui.menu_button("Diagnostics", |ui| {
                    diagnostics = self.diagnostics_menu(ui);
                });
            });
            ui.menu_button("Help", |ui| {
                self.item(ui, "Keyboard shortcuts", "F1", Shortcuts, commands);
                self.item(ui, "About unemu", "", About, commands);
            });
        });
        match diagnostics {
            Some(DiagnosticsItem::SaveReport) => self.save_diagnostics(),
            Some(DiagnosticsItem::RecordExceptions(on)) => self.set_record_exceptions(on),
            Some(DiagnosticsItem::OpenFolder) => self.open_diagnostics_folder(),
            None => {}
        }
    }

    /// Why Save machine report is unavailable now, in one short phrase.
    fn save_report_disabled(&self) -> Option<&'static str> {
        match (&self.presenter, &self.player) {
            (Some(_), _) => None,
            (None, Some(_)) => Some("A machine opened from a file has no report"),
            (None, None) => Some("Start a machine first"),
        }
    }

    /// Tools › Diagnostics: Save machine report, Record exceptions (a
    /// check item) and Open diagnostics folder; the item chosen.
    fn diagnostics_menu(&self, ui: &mut egui::Ui) -> Option<DiagnosticsItem> {
        let mut chosen = None;
        let save = ui.add_enabled(
            self.presenter.is_some(),
            egui::Button::new("Save machine report").shortcut_text(shell_model::shortcut_text(
                "Ctrl+Alt+Shift+D",
                self.macos_shortcuts,
            )),
        );
        let save = match self.save_report_disabled() {
            Some(reason) => save.on_disabled_hover_text(reason),
            None => save,
        };
        if save.clicked() {
            chosen = Some(DiagnosticsItem::SaveReport);
        }
        let mut record = self.record_exceptions;
        if ui.checkbox(&mut record, "Record exceptions").changed() {
            chosen = Some(DiagnosticsItem::RecordExceptions(record));
        }
        if ui.button("Open diagnostics folder").clicked() {
            chosen = Some(DiagnosticsItem::OpenFolder);
        }
        if chosen.is_some() {
            ui.close();
        }
        chosen
    }

    /// The PC's Machine, Media and View menus.
    fn pc_menus(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        use Command::*;
        ui.menu_button("Machine", |ui| {
            let target = match (
                &self.image_library.active_machine,
                self.image_library.selected_machine_row(),
            ) {
                (Some((_, name)), _) => format!("For the machine: {name} (running)"),
                (None, Some(row)) => format!("For the machine: {}", row.name()),
                (None, None) if self.presenter.is_some() => {
                    "A machine file or a gaming copy is running".into()
                }
                (None, None) => "Select a machine".into(),
            };
            ui.add_enabled(false, egui::Label::new(target));
            let reset = if self
                .presenter
                .as_ref()
                .is_some_and(|presenter| presenter.awaits_reset())
            {
                "Reset halted machine"
            } else {
                "Send Ctrl+Alt+Del"
            };
            for (mark, label, shortcut, command) in [
                (machine_menu_mark(Start), "Start", "Enter", Start),
                (machine_menu_mark(GamingMode), "Gaming mode", "", GamingMode),
                (
                    machine_menu_mark(Pause),
                    if self.paused { "Resume" } else { "Pause" },
                    "Ctrl+Alt+P",
                    Pause,
                ),
                (machine_menu_mark(Reset), reset, "Ctrl+Alt+End", Reset),
                (machine_menu_mark(PowerCycle), "Power cycle", "", PowerCycle),
                (machine_menu_mark(Stop), "Stop", "Ctrl+Alt+Q", Stop),
            ] {
                self.marked_item(ui, mark, label, shortcut, command, commands);
            }
            ui.separator();
            self.marked_item(
                ui,
                "",
                "Save state (not yet available)",
                "",
                SaveState,
                commands,
            );
            ui.separator();
            self.marked_item(ui, "", "Settings…", "Ctrl+,", Settings, commands);
            if self.presenter.is_none() && self.image_library.machine.is_some() {
                self.marked_item(ui, "", "Insert media…", "", Insert(0), commands);
            }
            self.marked_item(ui, "", "Disk layers…", "", Layers, commands);
            self.marked_item(ui, "", "Save as new image…", "", SaveImage, commands);
            ui.separator();
            self.marked_item(ui, "", "Duplicate…", "Ctrl+D", Duplicate, commands);
            self.marked_item(ui, "", "Rename…", "F2", Rename, commands);
            self.marked_item(ui, "", "Reset to image…", "", ResetMachine, commands);
            self.marked_item(ui, "", "Delete machine…", "Delete", DeleteMachine, commands);
            self.marked_item(ui, "", "Show source image", "", ShowSource, commands);
        });
        ui.menu_button("Media", |ui| {
            let drives = self.drive_views();
            let in_drives = self.floppy_inserted();
            for unit in 0..2 {
                let drive = if unit == 0 { "A:" } else { "B:" };
                let present = self.enablement().floppy[unit as usize];
                let view = drives.iter().find(|view| view.floppy_unit() == Some(unit));
                let inserted = view.is_some_and(|view| view.medium.is_some());
                let label = view.map_or_else(
                    || {
                        format!(
                            "Drive {drive} — {}",
                            if self.presenter.is_none() {
                                "machine stopped"
                            } else {
                                "not installed"
                            }
                        )
                    },
                    crate::drive_chips::DriveView::menu_label,
                );
                ui.add_enabled(present, egui::Label::new(label));
                let pending = view.is_some_and(|view| view.at_start);
                // One Insert… per drive opens the media picker.
                self.marked_item(
                    ui,
                    "",
                    "Insert…",
                    self.insert_shortcut(unit),
                    Insert(unit),
                    commands,
                );
                self.marked_item(
                    ui,
                    "",
                    "Create from files…",
                    "",
                    CreateFromFiles(unit),
                    commands,
                );
                self.catalogue_item(ui, unit, in_drives, false, commands);
                self.marked_item(
                    ui,
                    "",
                    "Next disk in catalogue",
                    if unit == 0 {
                        "Ctrl+Alt+F"
                    } else {
                        "Ctrl+Alt+Shift+F"
                    },
                    Next(unit),
                    commands,
                );
                ui.add_enabled_ui(inserted, |ui| {
                    self.marked_item(
                        ui,
                        EJECT_MARK,
                        &if pending {
                            "Take out again (pending)".to_owned()
                        } else {
                            format!("Eject {drive}")
                        },
                        eject_shortcut(unit),
                        Eject(unit),
                        commands,
                    );
                });
                if present && self.presenter.is_none() {
                    ui.add_enabled(
                        false,
                        egui::Label::new("Stopped: a disk inserted now goes in at the next Start."),
                    );
                }
                ui.separator();
            }
            let mut fixed = drives
                .iter()
                .filter(|view| view.floppy_unit().is_none())
                .map(crate::drive_chips::DriveView::menu_label)
                .peekable();
            if fixed.peek().is_none() {
                ui.add_enabled(
                    false,
                    egui::Label::new(if self.presenter.is_none() {
                        "C: — machine stopped"
                    } else {
                        "C: — no fixed disk"
                    }),
                );
            }
            for label in fixed {
                ui.add_enabled(false, egui::Label::new(label));
            }
            self.marked_item(ui, "", "Disk layers…", "", Layers, commands);
            ui.add_enabled(
                false,
                egui::Button::new("CD-ROM — unsupported (no controller)"),
            );
        });
        ui.menu_button("View", |ui| {
            ui.checkbox(&mut self.toolbar, "Toolbar");
            ui.checkbox(&mut self.statusbar, "Status bar");
            ui.separator();
            ui.selectable_value(&mut self.integer_scale, false, "Scale to window, 4:3");
            ui.selectable_value(&mut self.integer_scale, true, "Integer scale");
            self.item(ui, "Full screen", "Ctrl+Alt+Enter", Fullscreen, commands);
            self.item(ui, "Refresh remote", "F5", RefreshRemote, commands);
            let mute = self
                .presenter
                .as_ref()
                .map_or("Mute audio", |p| p.audio.mute_label());
            self.item(ui, mute, "Ctrl+Alt+M", Mute, commands);
            ui.menu_button("Speed", |ui| {
                let mut selected = self.selected_speed();
                for speed in crate::pacing::Speed::ALL {
                    let response = ui
                        .add_enabled_ui(self.enablement().allows(Speed(speed)), |ui| {
                            ui.selectable_value(&mut selected, speed, speed.label())
                        })
                        .inner;
                    if response.clicked() {
                        commands.push(Speed(speed));
                        ui.close();
                    }
                }
                ui.separator();
                ui.add_enabled(
                    false,
                    egui::Button::new("Hold for Unlimited").shortcut_text(
                        shell_model::shortcut_text("Ctrl+Alt+T", self.macos_shortcuts),
                    ),
                );
            });
            ui.menu_button("Skin", |ui| {
                ui.selectable_value(&mut self.classic, true, "Classic");
                ui.selectable_value(&mut self.classic, false, "Modern");
            });
            ui.menu_button("Theme", |ui| {
                egui::widgets::global_theme_preference_buttons(ui);
            });
        });
    }

    fn toolbar(&self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        ui.horizontal_wrapped(|ui| {
            use ui_kit::shell_icons::Icon;
            // The main-window mock-up sets Ctrl+Alt+Del as code text, without an icon.
            // A machine started from a file keeps the same buttons: the PC's
            // stay in place, greyed, and Ctrl+Alt+Del reads Reset.
            let player = self.player.as_ref();
            let paused = self.paused || player.is_some_and(player_ui::Player::paused);
            for (label, icon, command) in [
                (
                    if self.presenter.is_none() && player.is_none() {
                        self.image_library.start_label()
                    } else {
                        "Start"
                    },
                    Some(Icon::Play),
                    Command::Start,
                ),
                (
                    if paused { "Resume" } else { "Pause" },
                    Some(if paused { Icon::Play } else { Icon::Pause }),
                    Command::Pause,
                ),
                ("Stop", Some(Icon::Stop), Command::Stop),
                (
                    if player.is_some() {
                        "Reset"
                    } else {
                        "Ctrl+Alt+Del"
                    },
                    None,
                    Command::Reset,
                ),
                ("A:", Some(Icon::Floppy), Command::Insert(0)),
                ("B:", Some(Icon::Floppy), Command::Insert(1)),
                ("Eject", Some(Icon::Eject), Command::Eject(0)),
                ("Settings", Some(Icon::Settings), Command::Settings),
                ("Layers", Some(Icon::Layers), Command::Layers),
            ] {
                // Eject is the PC's button; the player's slots take the
                // media keys only.
                let pc_only = player.is_some() && matches!(command, Command::Eject(_));
                let response = ui
                    .add_enabled_ui(self.command_allowed(command) && !pc_only, |ui| {
                        toolbar_button(ui, self.classic, label, icon)
                    })
                    .inner;
                if self.classic {
                    bevel(ui, response.rect, response.is_pointer_button_down_on());
                }
                if response.clicked() {
                    commands.push(command);
                }
            }
            // The remotes mock-up's Refresh, which applies while the library
            // shows a registry's node, and Registries…, which opens
            // Preferences on its Registries page. While the library shows
            // Remotes its Pull… comes first, and takes the room of the speed
            // control, which applies to a running machine only: the toolbar
            // keeps to one line at the window's default width.
            let remotes = self.presenter.is_none()
                && self.player.is_none()
                && self.image_library.remote_shown();
            ui.separator();
            let pull = remotes.then_some(("Pull…", Icon::Pull, Command::Pull));
            // Push… beside Pull…: Push here… while a Remotes node is shown,
            // Push to… of the selected library items otherwise.
            let library = self.presenter.is_none() && self.player.is_none();
            let push = library.then_some((
                "Push…",
                Icon::Push,
                if remotes {
                    Command::PushHere
                } else {
                    Command::Push
                },
            ));
            for (label, icon, command) in pull.into_iter().chain(push).chain([
                ("Refresh", Icon::Refresh, Command::RefreshRemote),
                ("Registries…", Icon::Registry, Command::Preferences),
            ]) {
                let response = ui
                    .add_enabled_ui(self.command_allowed(command), |ui| {
                        toolbar_button(ui, self.classic, label, Some(icon))
                    })
                    .inner;
                if self.classic {
                    bevel(ui, response.rect, response.is_pointer_button_down_on());
                }
                let hint = match command {
                    Command::PushHere => {
                        Some("Push here…: library items to the selected registry or namespace")
                    }
                    Command::Push => Some("Push to…: the selected library items to a registry"),
                    _ => None,
                };
                let response = match hint {
                    Some(hint) => response.on_hover_text(hint),
                    None => response,
                };
                if response.clicked() {
                    commands.push(command);
                }
            }
            if !remotes {
                ui.separator();
                self.speed_selector(ui, commands);
            }
        });
    }

    /// The selection View › Speed checks; a held fast-forward leaves it unchanged.
    fn selected_speed(&self) -> crate::pacing::Speed {
        if let Some(player) = &self.player {
            return player.selected_speed();
        }
        self.presenter
            .as_ref()
            .map_or(crate::pacing::Speed::Normal, |p| p.pacing.selected())
    }

    /// The toolbar shows the mode in effect, so it reads Unlimited while the
    /// fast-forward chord is held and the selected mode again after release.
    fn toolbar_speed(&self) -> crate::pacing::Speed {
        if let Some(player) = &self.player {
            return player.selected_speed();
        }
        self.presenter
            .as_ref()
            .map_or(crate::pacing::Speed::Normal, |p| p.pacing.effective())
    }

    /// A classic toolbar combobox issuing the same command as View › Speed,
    /// so a change re-anchors pacing at the current guest clock.
    fn speed_selector(&self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let shown = self.toolbar_speed();
        let selected = self.selected_speed();
        ui.add_enabled_ui(self.command_allowed(Command::Speed(shown)), |ui| {
            ui.label("Speed:");
            let combo = egui::ComboBox::from_id_salt("toolbar-speed")
                .selected_text(shown.label())
                .width(88.0)
                .show_ui(ui, |ui| {
                    for speed in crate::pacing::Speed::ALL {
                        // The list marks the persistent choice; only the closed
                        // box reports a held fast-forward.
                        if ui
                            .selectable_label(speed == selected, speed.label())
                            .clicked()
                        {
                            commands.push(Command::Speed(speed));
                        }
                    }
                });
            if self.classic {
                bevel(ui, combo.response.rect, true);
            }
        });
    }

    fn title(&self) -> String {
        if let Some(player) = &self.player {
            player.title("unemu")
        } else if self.presenter.is_some() {
            match (
                &self.image_library.active_machine,
                &self.image_library.active,
            ) {
                // The machines mock-up names the machine and its image.
                (Some((_, name)), Some((row, _, _))) => {
                    format!("{name} — {} — unemu", row.reference)
                }
                _ => format!("{} — unemu", self.session_name().unwrap_or("Machine")),
            }
        } else {
            "unemu".into()
        }
    }

    fn skin(&self, ctx: &egui::Context) {
        let dark = ctx.global_style().visuals.dark_mode;
        ctx.set_visuals(skin_visuals(self.classic, dark));
    }

    /// The host picker for a firmware package's source archive; the library
    /// checks its signature and hash on the worker, never its name.
    fn pick_firmware_source(&mut self, target: crate::library_view::firmware::SourceTarget) {
        self.release();
        let package = self.image_library.firmware_edit_tag().map(str::to_owned);
        self.ask_file(FileFor::FirmwareSource(target, package));
    }

    /// The host picker for Import installation media…; the worker lists the result.
    fn pick_media(&mut self, pick: crate::library_view::install_media::Pick) {
        self.release();
        self.ask_file(FileFor::InstallMedia(pick));
    }

    fn welcome(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        // Always empty today: the library is drawn only while no machine runs (machine-images §5.2).
        self.image_library.running_drives = self
            .presenter
            .as_ref()
            .filter(|presenter| presenter.running)
            .map(|presenter| {
                (0u8..)
                    .zip(&presenter.media_slots)
                    .filter_map(|(unit, slot)| slot.as_ref().map(|slot| (unit, slot.geometry)))
                    .collect()
            })
            .unwrap_or_default();
        self.image_library.classic = self.classic;
        commands.extend(
            self.image_library
                .show(ui, &self.library, &mut self.selected),
        );
    }

    /// The player's screen in the central panel; a click on the picture
    /// gives the machine the keyboard.
    fn player_screen(&mut self, ui: &mut egui::Ui) {
        let Some(player) = &mut self.player else {
            return;
        };
        let clicked = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| player.screen(ui).clicked())
            .inner;
        self.screen = player.screen_rect().unwrap_or(egui::Rect::NOTHING);
        if clicked {
            self.focus_guest();
        }
    }

    fn display(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::BLACK))
            .show(ui, |ui| {
                let available = ui.available_size();
                let mut size = Vec2::new(
                    available.x.min(available.y * 4.0 / 3.0),
                    available.y.min(available.x * 3.0 / 4.0),
                );
                if self.integer_scale {
                    if let Some(texture) = &self.texture {
                        size = shell_model::integer_size(
                            available,
                            texture.size_vec2(),
                            unemu_pc_core::RASTER_SCALE,
                        );
                    }
                }
                let rect =
                    egui::Rect::from_center_size(ui.available_rect_before_wrap().center(), size);
                self.screen = rect;
                if let Some(texture) = &self.texture {
                    ui.painter().image(
                        texture.id(),
                        rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                let response = ui.interact(rect, ui.scope_id().with("guest-screen"), Sense::CLICK);
                if response.clicked() {
                    self.focus_guest();
                }
                if self.paused {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "PAUSED",
                        egui::FontId::proportional(20.0),
                        Color32::WHITE,
                    );
                }
            });
    }

    fn input(&mut self, ctx: &egui::Context, blocked: bool) {
        let (events, menu_commands, system_quit) = {
            let mut shared = self.events.borrow_mut();
            (
                std::mem::take(&mut shared.queue),
                shared.native.take_commands(),
                shared.native.take_system_quit(),
            )
        };
        for command in menu_commands {
            // Like a hotkey, a shell dialog holds back all but quitting. The
            // application menu also works while the window is minimized, so
            // bring forward what it opens.
            if command == Command::Exit || !self.has_dialog() {
                egui::Popup::close_all(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.command(command, ctx);
            }
        }
        if system_quit {
            self.command(Command::Exit, ctx);
        }
        for event in events {
            let blocked = blocked || self.has_dialog();
            let event = match event {
                HostEvent::Window(event) => *event,
                HostEvent::Key(event) => {
                    self.keyboard(event, blocked, ctx);
                    continue;
                }
                HostEvent::Motion(dx, dy) => {
                    if !blocked && self.guest_focus && !self.paused {
                        if let Some(presenter) = &mut self.presenter {
                            presenter.mouse_motion(dx, dy);
                        }
                        if let Some(player) = &mut self.player {
                            player.mouse_motion(dx, dy);
                        }
                    }
                    continue;
                }
            };
            if self.player.is_some() {
                self.player_window_event(event, blocked, ctx);
                continue;
            }
            match event {
                WindowEvent::ModifiersChanged(modifiers) if self.macos_shortcuts => {
                    self.modifiers_changed(modifiers.state());
                }
                WindowEvent::Focused(false) => self.focus_lost(),
                WindowEvent::Focused(true) => self.release(),
                WindowEvent::Occluded(hidden) => {
                    self.hidden = hidden;
                    if !hidden {
                        let elapsed = self.elapsed();
                        if let Some(presenter) = &mut self.presenter {
                            presenter.resync(elapsed);
                        }
                    }
                }
                WindowEvent::CursorMoved { position, .. } => {
                    self.pointer = egui::pos2(
                        position.x as f32 / ctx.pixels_per_point(),
                        position.y as f32 / ctx.pixels_per_point(),
                    );
                    if let Some(presenter) = &mut self.presenter {
                        presenter
                            .mouse
                            .pointer_inside(self.screen.contains(self.pointer));
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    if let Some(presenter) = &mut self.presenter {
                        presenter.mouse.pointer_inside(false);
                    }
                }
                WindowEvent::MouseInput { button, state, .. } => {
                    if !blocked
                        && self
                            .presenter
                            .as_ref()
                            .is_some_and(|presenter| presenter.mouse.captured())
                    {
                        if let Some(presenter) = &mut self.presenter {
                            presenter.mouse_button(button, state);
                        }
                    } else if !blocked
                        && self.screen.contains(self.pointer)
                        && self.presenter.is_some()
                    {
                        self.focus_guest();
                        if let Some(presenter) = &mut self.presenter {
                            presenter.focus_changed(true);
                            presenter.mouse.pointer_inside(true);
                            presenter.mouse_button(button, state);
                        }
                    } else if state == ElementState::Pressed {
                        self.release();
                    } else if let Some(presenter) = &mut self.presenter {
                        if presenter.mouse.captured() {
                            presenter.mouse_button(button, state);
                        }
                    }
                }
                _ => {}
            }
        }
        self.sync_routing();
        self.sync_system_keys();
        // Every pass reads input, so a cancelled or abandoned question is
        // answered here, after the Exit that a system quit asked for has run.
        self.answer_system_quit(self.exit_progress(), ctx);
    }

    /// A window event while a machine started from a file runs: the same
    /// routing as for the PC, to the player.
    fn player_window_event(&mut self, event: WindowEvent, blocked: bool, ctx: &egui::Context) {
        match event {
            WindowEvent::ModifiersChanged(modifiers) if self.macos_shortcuts => {
                self.modifiers_changed(modifiers.state());
            }
            WindowEvent::Focused(false) => self.focus_lost(),
            WindowEvent::Focused(true) => self.release(),
            // The player's tick holds the machine while the window is hidden.
            WindowEvent::Occluded(hidden) => self.hidden = hidden,
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = egui::pos2(
                    position.x as f32 / ctx.pixels_per_point(),
                    position.y as f32 / ctx.pixels_per_point(),
                );
                let inside = self.screen.contains(self.pointer);
                if let Some(player) = &mut self.player {
                    player.pointer_inside(inside);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(player) = &mut self.player {
                    player.pointer_inside(false);
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let captured = self
                    .player
                    .as_ref()
                    .is_some_and(player_ui::Player::pointer_captured);
                if !blocked && !captured && self.screen.contains(self.pointer) {
                    self.focus_guest();
                    if let Some(player) = &mut self.player {
                        player.pointer_inside(true);
                    }
                } else if (blocked || !captured) && state == ElementState::Pressed {
                    self.release();
                    return;
                }
                // A captured pointer's buttons, a click on the screen, and
                // every release, so no button stays held.
                if let Some(player) = &mut self.player {
                    player.mouse_button(button, state);
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    fn key(&mut self, key: KeyCode, state: ElementState, blocked: bool, ctx: &egui::Context) {
        self.keyboard(
            HostKey {
                key,
                shortcut_key: Some(key),
                state,
                repeat: self.held.contains(&key) && state == ElementState::Pressed,
                synthetic: false,
                guest_focus: self.guest_focus,
                session: self.session,
            },
            blocked,
            ctx,
        );
    }

    fn send_guest_key(&mut self, key: KeyCode, state: ElementState) {
        if let Some(player) = &mut self.player {
            if state == ElementState::Pressed {
                self.guest_held.insert(key);
            } else if !self.guest_held.remove(&key) {
                return;
            }
            // As for the PC: on macOS the keys go past the session's host
            // chords, which the shell's Command shortcuts replace.
            if self.macos_shortcuts {
                player.guest_key(key, state);
            } else {
                player.key_event(key, state);
            }
            return;
        }
        if let Some(presenter) = &mut self.presenter {
            if state == ElementState::Pressed {
                self.guest_held.insert(key);
            } else if !self.guest_held.remove(&key) {
                return;
            }
            if self.macos_shortcuts {
                // The legacy presenter reserves Ctrl+Alt for media and mouse
                // controls; macOS must pass those chords intact to the guest.
                presenter.guest_key(key, state);
            } else {
                presenter.key_event(key, state);
            }
        }
    }

    fn consume_hotkey(&mut self, key: KeyCode) {
        self.consumed.insert(key);
        if self.macos_shortcuts {
            return;
        }
        // A modifier already used by an earlier guest chord needs its break
        // before that same physical key becomes part of a host shortcut.
        let modifiers = self
            .held
            .iter()
            .copied()
            .filter(|key| modifier_index(*key).is_some())
            .collect::<Vec<_>>();
        for key in modifiers {
            self.send_guest_key(key, ElementState::Released);
            self.consumed.insert(key);
        }
    }

    fn keyboard(&mut self, event: HostKey, blocked: bool, ctx: &egui::Context) {
        let HostKey {
            key,
            shortcut_key,
            state,
            repeat,
            synthetic,
            guest_focus,
            session,
        } = event;
        if session != self.session {
            return;
        }
        let blocked = blocked || self.has_dialog();
        let pressed = state == ElementState::Pressed;
        let command_modifier =
            self.macos_shortcuts && matches!(key, KeyCode::SuperLeft | KeyCode::SuperRight);
        if pressed && (synthetic || guest_focus != self.guest_focus) {
            return;
        }
        let fresh = if pressed {
            if repeat && !self.held.contains(&key) {
                return;
            }
            self.held.insert(key)
        } else {
            self.held.remove(&key);
            false
        };
        if let Some(index) = modifier_index(key) {
            if !pressed || fresh {
                self.modifiers[index] = pressed;
            }
            if self.macos_shortcuts {
                let flag = match index {
                    0 | 1 => ModifiersState::CONTROL,
                    2 | 3 => ModifiersState::ALT,
                    4 | 5 => ModifiersState::SHIFT,
                    _ => ModifiersState::SUPER,
                };
                let first = index / 2 * 2;
                let mut modifiers = self.host_modifiers;
                modifiers.set(flag, self.modifiers[first..first + 2].contains(&true));
                self.modifiers_changed(modifiers);
            }
        }
        if command_modifier {
            return;
        }
        // The Input dialog takes the next key pressed, raw, so a key such as
        // the right Shift can be bound; nothing reaches the machine.
        if fresh {
            if let Some(player) = self.player.as_mut().filter(|player| player.has_dialog()) {
                player.key_event(key, state);
            }
        }
        if self.consumed.contains(&key) {
            if !pressed {
                if self.fast_forward_key == Some(key) {
                    self.fast_forward_key = None;
                    self.end_fast_forward();
                }
                self.consumed.remove(&key);
            }
            return;
        }
        let shortcut_key = if self.macos_shortcuts {
            shortcut_key
        } else {
            Some(key)
        };
        let mut modifiers = self.modifiers;
        if self.macos_shortcuts {
            modifiers = [
                self.host_modifiers.control_key(),
                false,
                self.host_modifiers.alt_key(),
                false,
                self.host_modifiers.shift_key(),
                false,
                self.host_modifiers.super_key(),
                false,
            ];
        }
        if fresh
            && self.presenter.is_some()
            && shortcut_key.is_some_and(|shortcut_key| {
                crate::diagnostics::is_chord(shortcut_key, modifiers, self.macos_shortcuts)
            })
        {
            self.consume_hotkey(key);
            self.save_diagnostics();
            return;
        }
        if fresh {
            if let Some(command) = shortcut_key
                .and_then(|shortcut_key| {
                    shell_model::hotkey(
                        shortcut_key,
                        modifiers,
                        self.state(),
                        self.guest_focus,
                        self.macos_shortcuts,
                    )
                })
                // Quitting is never held back by an open dialog or menu.
                .filter(|command| !blocked || *command == Command::Exit)
                // Save state and Load state are chords of a machine that
                // saves state; for the PC and any other machine they stay
                // guest input.
                .filter(|command| {
                    !matches!(command, Command::SaveState | Command::LoadState)
                        || self
                            .player
                            .as_ref()
                            .is_some_and(|player| player.offers().saved_state)
                })
                // An edited text field keeps Enter, Delete and F2.
                .filter(|command| !(command.library_key() && ctx.egui_wants_keyboard_input()))
            {
                self.consume_hotkey(key);
                if command == Command::FastForward {
                    self.fast_forward_key = Some(key);
                }
                self.command(command, ctx);
                return;
            }
        }
        // Under Command only Ctrl stays guest input. An Option pressed for a
        // chord such as Command+Option+H would reach the guest as an Alt tap,
        // which opens the menu bar in many DOS programs.
        if self.macos_shortcuts
            && self.host_modifiers.super_key()
            && !matches!(key, KeyCode::ControlLeft | KeyCode::ControlRight)
        {
            if fresh {
                self.consumed.insert(key);
            } else if !pressed {
                self.send_guest_key(key, state);
            }
            return;
        }
        if !self.guest_focus {
            return;
        }
        if !pressed || (!blocked && !self.paused && (fresh || self.guest_held.contains(&key))) {
            self.send_guest_key(key, state);
        }
    }

    fn cursor(&self, ctx: &egui::Context) {
        if self
            .presenter
            .as_ref()
            .is_some_and(|presenter| presenter.mouse.captured())
            || self
                .player
                .as_ref()
                .is_some_and(player_ui::Player::pointer_captured)
        {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }

    fn visibility(&mut self, ctx: &egui::Context, minimized: bool) {
        self.minimized = minimized;
        let lifecycle = self.events.borrow().queue.iter().any(|event| matches!(event,
            HostEvent::Window(event) if matches!(**event, WindowEvent::Focused(false) | WindowEvent::Occluded(_))));
        if minimized || self.hidden || lifecycle {
            self.input(ctx, true);
            if minimized {
                self.focus_lost();
            }
            self.tick(ctx, egui::Popup::is_any_open(ctx), self.elapsed());
        }
    }

    fn tick(&mut self, ctx: &egui::Context, blocked: bool, elapsed: std::time::Duration) {
        let tick_started = Instant::now();
        let has_dialog = self.has_dialog();
        if let Some(player) = &mut self.player {
            // The player holds its machine for its own dialogs and pause; the
            // shell's dialogs, a menu and a hidden window hold it too.
            let hold = blocked || has_dialog || self.hidden || self.minimized;
            #[cfg(test)]
            let warnings = &mut self.player_log;
            #[cfg(not(test))]
            let warnings = &mut io::stderr();
            player.tick(ctx, hold, elapsed, warnings);
            return;
        }
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        if presenter.takes_media() {
            if let Err(error) = presenter.apply_media_events() {
                self.show_error(error.to_string());
                return;
            }
        }
        let pause = self.paused || blocked || has_dialog || self.hidden || self.minimized;
        if pause && !self.modal_paused {
            presenter.occlusion_changed(true, elapsed);
        }
        if !pause && self.modal_paused {
            presenter.occlusion_changed(false, elapsed);
        }
        self.modal_paused = pause;
        let (advance, deadline) = crate::pacing::wake(
            elapsed,
            presenter.next_paint,
            presenter.running && presenter.occluded_since.is_none(),
        );
        presenter.next_paint = deadline;
        if advance {
            if let Err(error) = presenter.advance(elapsed) {
                self.show_error(error.to_string());
                return;
            }
            presenter.observe_drive_lights(elapsed);
            if let Some(frame) = presenter.picture() {
                let image = shell_model::texture_image(frame);
                if let Some(texture) = &mut self.texture {
                    texture.set(image, egui::TextureOptions::NEAREST);
                } else {
                    self.texture = Some(ctx.load_texture(
                        "guest-screen",
                        image,
                        egui::TextureOptions::NEAREST,
                    ));
                }
            }
            if let Some(error) = &presenter.error {
                let error = error.to_string();
                let report = presenter
                    .machine()
                    .cpu_stop_report()
                    .map(ToString::to_string);
                let report = report.map(|text| self.saved_report(text));
                self.show_machine_failure(error, report);
                return;
            }
            if !presenter.running {
                self.message = if presenter.halted {
                    "Machine halted — insert a boot disk and press Ctrl+Alt+Del, or Stop to return to the library"
                } else {
                    "Machine halted — Stop returns to the library"
                }
                .into();
                self.paused = true;
            }
        }
        if let Some(deadline) = deadline {
            ctx.request_repaint_after(
                deadline.saturating_sub(elapsed.saturating_add(tick_started.elapsed())),
            );
        }
        if presenter.audio.check_failure(&mut io::stderr()) {
            self.message = "Host audio unavailable".into();
        }
    }
}

impl Drop for Shell {
    fn drop(&mut self) {
        if let Some(job) = &self.readiness {
            job.control.cancel();
        }
        self.image_library.shutdown();
        // Quitting with a machine running still writes its recording.
        self.finish_recording();
    }
}

/// An item of Tools › Diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticsItem {
    SaveReport,
    RecordExceptions(bool),
    OpenFolder,
}

/// Write `presenter`'s exception recording to its file in `folder` and end
/// it; the status line's words for it, `None` when nothing recorded.
fn recording_finished(
    folder: &Result<std::path::PathBuf, String>,
    presenter: &mut Presenter,
) -> Option<String> {
    Some(match presenter.finish_recording(folder)? {
        Ok(path) => format!("Exceptions recorded to {}", path.display()),
        Err(error) => {
            eprintln!("exceptions not written: {error}");
            format!("Exceptions not written: {error}")
        }
    })
}

fn firmware_progress(
    ui: &mut egui::Ui,
    progress: &machine_file::firmware_discovery::DiscoveryProgress,
    reduced_motion: bool,
) {
    use machine_file::firmware_discovery::DiscoveryPhase;
    if progress.phase == DiscoveryPhase::Idle {
        return;
    }
    let text = match progress.phase {
        DiscoveryPhase::Loading => "Loading machine…".into(),
        DiscoveryPhase::Cancelled => "Firmware check cancelled · Check again uses the cache".into(),
        DiscoveryPhase::Complete => {
            format!("Firmware check complete · {} matches", progress.matches)
        }
        _ => format!(
            "Scanning ROM folders… {} files listed · {}/{} candidates hashed · {} matches",
            progress.listed, progress.checked, progress.candidates, progress.matches
        ),
    };
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(0.0), 14.0),
        Sense::hover(),
    );
    let bar = egui::Rect::from_min_size(
        rect.min,
        Vec2::new(96.0_f32.min(rect.width() * 0.3), rect.height()),
    );
    bevel(ui, bar, true);
    let inner = bar.shrink(3.0);
    let indeterminate = matches!(
        progress.phase,
        DiscoveryPhase::Listing | DiscoveryPhase::Loading
    );
    let filled = if indeterminate {
        4
    } else if progress.candidates == 0 {
        0
    } else {
        (progress.checked * 16)
            .div_ceil(progress.candidates)
            .min(16)
    };
    let offset = if indeterminate && !reduced_motion {
        (ui.input(|input| input.time) * 8.0) as usize % 16
    } else {
        0
    };
    for index in 0..16 {
        if (index + 16 - offset) % 16 < filled {
            let width = inner.width() / 16.0;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(inner.left() + index as f32 * width, inner.top()),
                    Vec2::new((width - 2.0).max(0.0), inner.height().max(0.0)),
                ),
                0.0,
                ui.visuals().selection.bg_fill,
            );
        }
    }
    let text_rect = egui::Rect::from_min_max(
        egui::pos2((bar.right() + 6.0).min(rect.right()), rect.top()),
        rect.max,
    );
    let mut job = egui::text::LayoutJob::simple_singleline(
        text.clone(),
        egui::FontId::proportional(12.0),
        ui.visuals().text_color(),
    );
    job.wrap.max_width = text_rect.width();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter()
        .with_clip_rect(text_rect.intersect(ui.clip_rect()))
        .galley(text_rect.min, galley, ui.visuals().text_color());
    response.on_hover_text(text);
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ui_kit::text_clipboard::shortcuts(ctx, &mut *self.clipboard);
        // Before anything draws: a file dialog opened later in this frame
        // then already wakes the shell with its answer.
        self.poll_file_dialog(ctx);
        self.poll_opening();
        self.poll_images(ctx);
        self.poll_readiness(ctx);
        self.sync_pending_media();
        self.visibility(
            ctx,
            ctx.input(|input| input.viewport().minimized == Some(true)),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.window.is_none() {
            self.window = frame.winit_window().cloned();
        }
        self.skin(&ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.title()));
        let mut commands = Vec::new();
        egui::Panel::top("shell-chrome").show(ui, |ui| {
            if ui_kit::shell_icons::custom_title_bar(self.classic, cfg!(target_os = "macos")) {
                window_title(ui, &self.title());
            }
            self.menus(ui, &mut commands);
            if self.toolbar {
                ui.separator();
                self.toolbar(ui, &mut commands);
            }
        });
        if let Some(player) = &mut self.player {
            player.set_look(self.classic, self.macos_shortcuts);
        }
        if self.statusbar {
            egui::Panel::bottom("statusbar").show(ui, |ui| match &self.player {
                Some(player) => player.status(ui),
                None => self.status(ui, &mut commands),
            });
        }
        self.screen = egui::Rect::NOTHING;
        if self.player.is_some() {
            self.player_screen(ui);
        } else if self.presenter.is_some() {
            self.display(ui);
        } else {
            self.welcome(ui, &mut commands);
        }
        let menu_open = egui::Popup::is_any_open(&ctx);
        let had_dialog = self.has_dialog();
        // The stop prompt is drawn last, above any other dialog; it takes its
        // Escape before the dialogs beneath it can read the same key.
        let stop_escape = self.stop_prompt.is_some() && ctx.input_mut(escape_pressed);
        let transfer_escape = self.transfer_prompt.is_some() && ctx.input_mut(escape_pressed);
        let mut close_dialog = false;
        let mut open_machine = false;
        let mut save_preferences = false;
        let mut rescan_firmware = false;
        let mut recheck_firmware = false;
        let mut cancel_firmware = false;
        let mut browse_rom_folder = false;
        let mut reattach_remotes = false;
        let show_firmware_progress = self.visible_discovery() || self.discovery_finished;
        if let Some((title, text)) = &self.dialog {
            egui::Modal::new(egui::Id::unique("shell-dialog")).show(&ctx, |ui| {
                ui.set_max_width(600.0);
                if self.classic {
                    gradient_title(ui, title);
                } else {
                    ui.heading(*title);
                }
                if *title == "About unemu" {
                    if let Some(logo) = &self.logo {
                        logo.about(ui);
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.label(text);
                    });
                // Copy takes the message, and the report under it.
                let mut copy = text.clone();
                if let Some(report) = self
                    .error_report
                    .as_ref()
                    .filter(|_| *title == "Machine error")
                {
                    ui.label(&report.saved);
                    egui::ScrollArea::both()
                        .id_salt("cpu-stop-report")
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&report.text).monospace())
                                    .extend(),
                            );
                        });
                    copy = format!("{text}\n\n{}", report.text);
                }
                if *title == "Open machine file" {
                    ui.label("Machine file path (.toml, .yaml, .yml)");
                    ui.text_edit_singleline(&mut self.open_path);
                    if ui
                        .add_enabled(!self.open_path.trim().is_empty(), egui::Button::new("Open"))
                        .clicked()
                    {
                        open_machine = true;
                    }
                }
                if *title == "ROM folders" {
                    ui.label("Image store");
                    let mut root = self
                        .preferences_draft
                        .store_root
                        .clone()
                        .or_else(|| image_store::location::default_store_root().ok())
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if ui.text_edit_singleline(&mut root).changed() {
                        self.preferences_draft.store_root =
                            (!root.trim().is_empty()).then(|| std::path::PathBuf::from(root));
                    }
                    let mut remove = None;
                    for (index, path) in self.preferences_draft.rom_dirs.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(path.to_string_lossy());
                            if ui.button("Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                    if let Some(index) = remove {
                        self.preferences_draft.rom_dirs.remove(index);
                    }
                    ui.text_edit_singleline(&mut self.rom_folder_input);
                    ui.horizontal(|ui| {
                        if ui.button("Add folder").clicked() {
                            match self
                                .preferences_draft
                                .add_folder(std::path::Path::new(&self.rom_folder_input))
                            {
                                Ok(()) => self.rom_folder_input.clear(),
                                Err(error) => self.message = format!("ROM folder: {error}"),
                            }
                        }
                        if ui.button("Browse…").clicked() {
                            browse_rom_folder = true;
                        }
                    });
                    ui.separator();
                    let progress = self.discovery_control.snapshot();
                    if show_firmware_progress {
                        firmware_progress(ui, &progress, self.preferences.reduced_motion);
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(self.selected.is_some(), egui::Button::new("Check again"))
                            .clicked()
                        {
                            recheck_firmware = true;
                        }
                        if ui
                            .add_enabled(self.selected.is_some(), egui::Button::new("Rescan"))
                            .clicked()
                        {
                            rescan_firmware = true;
                        }
                        if ui
                            .add_enabled(self.readiness.is_some(), egui::Button::new("Cancel scan"))
                            .clicked()
                        {
                            cancel_firmware = true;
                        }
                    });
                    ui.label(format!(
                        "{} found · {} results (all matches, up to 256 skips)",
                        progress.matches, progress.result_count
                    ));
                    egui::ScrollArea::vertical()
                        .id_salt("firmware-results")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for result in &progress.results {
                                ui.add(egui::Label::new(result).wrap());
                            }
                        });
                    if !self.discovery_warnings.is_empty() {
                        ui.label(format!(
                            "{} skipped firmware search entries",
                            self.discovery_warnings.len()
                        ));
                        for warning in self.discovery_warnings.iter().take(3) {
                            ui.add(egui::Label::new(warning).wrap());
                        }
                    }
                    ui.label("UNEMUPC_USER_ROMS_DIR is searched as well when set.");
                    if let Some(path) = &self.preferences_path {
                        ui.label(format!("Preferences file: {}", path.display()));
                    }
                    if ui.button("Save").clicked() {
                        save_preferences = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                } else if *title == "Missing firmware" {
                    if ui.button("Locate firmware…").clicked() {
                        commands.push(Command::FirmwareFolders);
                    }
                    if message_buttons(ui, &copy) {
                        close_dialog = true;
                    }
                } else if *title == "Image store folder" {
                    ui.text_edit_singleline(&mut self.store_path_input);
                    if ui
                        .add_enabled(
                            self.presenter.is_none()
                                && self.player.is_none()
                                && !self.image_library.busy,
                            egui::Button::new("Save"),
                        )
                        .clicked()
                    {
                        let path = std::path::PathBuf::from(self.store_path_input.trim());
                        if path.is_absolute() {
                            self.preferences.store_root = Some(path.clone());
                            if let Some(config) = &self.preferences_path {
                                if let Err(e) = self.preferences.save(config) {
                                    self.message = e.to_string();
                                } else {
                                    self.image_library =
                                        crate::library_view::LibraryView::new(Some(path))
                                            .with_compatibility(
                                                platforms::compatibility(env!("CARGO_PKG_VERSION"))
                                                    .unwrap_or_default(),
                                            );
                                    reattach_remotes = true;
                                    close_dialog = true;
                                }
                            }
                        } else {
                            self.message = "Store folder must be an absolute local path".into();
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close_dialog = true;
                    }
                } else if message_buttons(ui, &copy) {
                    close_dialog = true;
                }
            });
        }
        self.media_modals(&ctx);
        self.picker_modal(&ctx);
        self.settings_modal(&ctx);
        if let Some(dialog) = &mut self.preferences_dialog {
            match crate::preferences_sheet::show(&ctx, dialog, self.classic) {
                crate::preferences_sheet::Action::None => {}
                crate::preferences_sheet::Action::Close => self.close_preferences(),
                crate::preferences_sheet::Action::FirmwareFolders => {
                    self.close_preferences();
                    commands.push(Command::FirmwareFolders);
                }
                crate::preferences_sheet::Action::SystemShortcuts(on) => {
                    self.set_system_shortcuts(on)
                }
            }
        }
        let path = self
            .selected
            .and_then(|i| self.library.get(i))
            .map(|e| e.path.clone());
        let roots = self
            .preferences
            .roots(&self.args.rom_dirs, &self.default_firmware_roots)
            .into_iter()
            .map(|r| r.path)
            .collect();
        let image_actions = self.image_library.modals(
            &ctx,
            path.as_deref(),
            roots,
            self.classic,
            self.presenter.is_some(),
        );
        self.library_commands(&mut commands);
        if let Some(pick) = self.image_library.media_pick.take() {
            self.pick_media(pick);
        }
        if let Some(target) = self.image_library.firmware_source_pick.take() {
            self.pick_firmware_source(target);
        }
        for action in image_actions {
            if matches!(
                action,
                crate::library_view::Request::Publish(..)
                    | crate::library_view::Request::Discard(_)
                    | crate::library_view::Request::Machine(
                        crate::library_worker::MachineJob::Commit { .. }
                    )
            ) && !self.stop_for_image_action(&action)
            {
                continue;
            }
            self.image_library.send(action);
        }
        if let Some(player) = &mut self.player {
            let request = player.dialogs(&ctx);
            self.player_request(request, &ctx);
        }
        if let Some(refusal) = &self.open_refusal {
            if refusal.show(&ctx, self.classic) {
                self.open_refusal = None;
            }
        }
        if let Some(stop) = self.stop_prompt_modal(&ctx, stop_escape) {
            self.answer_stop_prompt(stop, &ctx);
        }
        if let Some(quit) = self.transfer_prompt_modal(&ctx, transfer_escape) {
            self.answer_transfer_prompt(quit, &ctx);
        }
        if open_machine {
            self.open_machine();
        }
        if save_preferences {
            self.save_preferences();
        }
        if recheck_firmware {
            self.check_firmware_again();
        }
        if rescan_firmware {
            self.rescan_firmware();
        }
        if cancel_firmware {
            self.cancel_firmware();
        }
        if browse_rom_folder {
            self.ask_file(FileFor::RomFolder);
        }
        if reattach_remotes {
            // The new store's library lists the same registries.
            let accounts = self.registry_accounts();
            self.image_library.attach_remotes(accounts);
        }
        if close_dialog {
            if self
                .dialog
                .as_ref()
                .is_some_and(|(title, _)| *title == "ROM folders")
            {
                self.close_firmware_dialog();
            } else {
                self.dialog = None;
            }
        }
        if menu_open || had_dialog {
            self.release();
        }
        let player_commands = std::mem::take(&mut self.player_commands);
        self.input(
            &ctx,
            menu_open || had_dialog || !commands.is_empty() || !player_commands.is_empty(),
        );
        for command in commands {
            self.command(command, &ctx);
        }
        for command in player_commands {
            self.player_command(command, &ctx);
        }
        let blocked = menu_open || had_dialog || self.has_dialog();
        if blocked {
            self.release();
        }
        self.resume_input(blocked);
        self.tick(&ctx, blocked, self.elapsed());
        self.sync_routing();
        self.sync_system_keys();
        {
            let mut events = self.events.borrow_mut();
            events.screen = if blocked || (self.presenter.is_none() && self.player.is_none()) {
                None
            } else {
                Some(self.screen)
            };
            events.pixels_per_point = ctx.pixels_per_point();
        }
        outline_focused_field(&ctx);
        self.cursor(&ctx);
        self.window_close(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop();
        self.image_library.shutdown();
    }
}

/// Outlines the focused text field in its own text colour, as the mock-ups
/// outline a focused control (`:focus-visible`: 1 px in `currentColor`), in
/// every skin. egui outlines it in the selection stroke, which stays white so
/// that selected glyphs read on the selection fill; white all but vanishes on
/// the light field. Called once a pass after everything is drawn, it paints
/// over egui's outline in the field's own layer, clipped as the field is.
pub(crate) fn outline_focused_field(ctx: &egui::Context) {
    let Some(id) = ctx.memory(|memory| memory.focused()) else {
        return;
    };
    if egui::TextEdit::load_state(ctx, id).is_none() {
        return;
    }
    let Some(field) = ctx.read_response(id) else {
        return;
    };
    let style = ctx.global_style();
    let visuals = &style.visuals;
    let outline = Stroke::new(
        visuals.selection.stroke.width,
        visuals
            .override_text_color
            .unwrap_or(visuals.widgets.inactive.text_color()),
    );
    ctx.layer_painter(field.layer_id)
        .with_clip_rect(field.interact_rect)
        .rect_stroke(
            field.rect,
            visuals.widgets.active.corner_radius,
            outline,
            egui::StrokeKind::Inside,
        );
}

fn modifier_index(key: KeyCode) -> Option<usize> {
    match key {
        KeyCode::ControlLeft => Some(0),
        KeyCode::ControlRight => Some(1),
        KeyCode::AltLeft => Some(2),
        KeyCode::AltRight => Some(3),
        KeyCode::ShiftLeft => Some(4),
        KeyCode::ShiftRight => Some(5),
        KeyCode::SuperLeft => Some(6),
        KeyCode::SuperRight => Some(7),
        _ => None,
    }
}
fn led(ui: &mut egui::Ui, on: bool, label: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 6.0), Sense::hover());
    ui.painter().rect_filled(
        rect,
        0.0,
        if on {
            Color32::from_rgb(54, 210, 74)
        } else {
            Color32::from_rgb(74, 74, 68)
        },
    );
    ui.label(label);
}
/// The Machine menu's mark column, as the main-window mock-up draws it.
pub(crate) fn machine_menu_mark(command: Command) -> &'static str {
    match command {
        Command::Start => "▶",
        Command::Pause => "Ⅱ",
        Command::Stop => "■",
        _ => "",
    }
}

/// Why a stopped library machine lists no disk in its drives' catalogues:
/// its image's own disks join them at Start.
const IMAGE_DISKS_AT_START: &str =
    "The image's own disks are in the drive at Start; Next disk swaps them once the machine runs.";

/// The disks waiting for a stopped drive's Start and the place of the one
/// that goes in.
#[derive(Default)]
struct Waiting {
    disks: Vec<crate::media_picker::CatalogueDisk>,
    chosen: Option<usize>,
    /// The catalogue a library machine kept at its last Stop is among them,
    /// its disks numbered from here on (after a pending insert's).
    kept: Option<usize>,
}

fn eject_shortcut(unit: u8) -> &'static str {
    if unit == 0 {
        "Ctrl+Alt+E"
    } else {
        "Ctrl+Alt+Shift+E"
    }
}
/// The amber drive light of the approved status-bar design, or its dark off state.
fn disk_led_color(ui: &egui::Ui, on: bool) -> Color32 {
    if on {
        Color32::from_rgb(255, 178, 31)
    } else if ui.visuals().dark_mode {
        Color32::from_rgb(42, 42, 39)
    } else {
        Color32::from_rgb(74, 74, 68)
    }
}
/// A stopped machine's drives that hold a disk for its next Start, as the
/// status bar and the Media menu show them.
fn machine_pending_views(
    row: &crate::library_view::machines::MachineRow,
    label: impl Fn(&image_store::machine::PendingInsert) -> String,
) -> Vec<crate::drive_chips::DriveView> {
    row.drives
        .iter()
        .filter_map(|&(unit, geometry)| {
            let letter = crate::media_picker::drive_name(unit);
            let insert = row
                .listing
                .machine
                .pending
                .iter()
                .find(|p| p.drive == letter);
            let medium = match insert {
                Some(insert) => crate::drive_chips::Medium {
                    name: label(insert),
                    source: format!(
                        "Pending insert from {}, in the drive at the next Start of {}",
                        insert.reference,
                        row.name()
                    ),
                    sha256: None,
                    write: if insert.journal {
                        assembly::WriteMode::journal(std::path::PathBuf::from(&insert.medium))
                    } else {
                        assembly::WriteMode::ReadOnly
                    },
                    fixed_by: Some("chosen in the media picker"),
                    format: None,
                    set: None,
                },
                // Created disks the machine owns wait in its workspace.
                None => {
                    let created = row.created.iter().find(|c| c.drive == letter)?;
                    let count = created.disks.len();
                    // The disk that goes in, by the name its catalogue gives it.
                    let first = created
                        .disks
                        .get(created.first())
                        .map_or_else(|| created.title.clone(), |disk| disk.name.clone());
                    crate::drive_chips::Medium {
                        name: if count > 1 {
                            format!("{first} (all {count} disks join the catalogue)")
                        } else {
                            first
                        },
                        source: format!(
                            "Created from files, in the drive at the next Start of {}",
                            row.name()
                        ),
                        sha256: None,
                        write: if created.journal {
                            assembly::WriteMode::journal(std::path::PathBuf::from(&created.title))
                        } else {
                            assembly::WriteMode::ReadOnly
                        },
                        fixed_by: Some("created in the media picker"),
                        format: None,
                        set: None,
                    }
                }
            };
            Some(crate::drive_chips::DriveView {
                bay: crate::drive_chips::Bay::Floppy { unit, geometry },
                medium: Some(medium),
                from_image: true,
                at_start: true,
            })
        })
        .collect()
}

fn drive_tooltip(ui: &mut egui::Ui, view: &crate::drive_chips::DriveView, lit: bool) {
    ui.set_max_width(360.0);
    egui::Grid::new(("drive-tooltip", view.slot()))
        .num_columns(2)
        .show(ui, |ui| {
            for (key, value) in view.tooltip(lit) {
                ui.weak(key);
                ui.add(egui::Label::new(value).wrap());
                ui.end_row();
            }
        });
    ui.separator();
    ui.weak("Click for this drive's menu");
}
/// One main-toolbar button. The classic skin frames it, and the toolbar
/// bevels it. The modern skin draws it as the main-window mock-up does: no
/// frame at rest, and on hover only the skin's 1 px outline, without a fill.
/// It changes the hover look of the `Ui` it is given, so it gets one of its
/// own, as the toolbar's `add_enabled_ui` does.
/// A command of one library item, which a selection of several rows
/// leaves unavailable.
fn single_item(command: Command) -> bool {
    use Command::*;
    matches!(
        command,
        Start
            | GamingMode
            | ImageStart
            | ImageGaming
            | Settings
            | Layers
            | SaveImage
            | NewFromImage
            | ImageSettings
            | ImageLayers
            | DeleteTag
            | DeleteSelection
            | EditFirmware
            | InsertSet(_)
    )
}

fn toolbar_button(
    ui: &mut egui::Ui,
    classic: bool,
    label: &str,
    icon: Option<ui_kit::shell_icons::Icon>,
) -> egui::Response {
    if !classic {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.hovered.weak_bg_fill = Color32::TRANSPARENT;
        widgets.hovered.bg_stroke = widgets.inactive.bg_stroke;
    }
    ui_kit::shell_icons::button(ui, label, icon, classic)
}

/// Shown in Preferences when the system-shortcut option is on without the
/// Accessibility privilege.
const ACCESS_NOTE: &str = "Allow unemu, or the terminal that started it, in System \
    Settings › Privacy & Security › Accessibility. Until then these shortcuts stay with macOS.";

/// Hands the keys the keyboard tap takes from macOS to the window's key
/// queue, as if the window had received them; refuses them once the guest
/// lost the keyboard, so the tap passes them on to macOS.
fn system_key_sink(events: Rc<RefCell<Events>>) -> crate::system_keys::Deliver {
    Box::new(move |key| {
        let Ok(mut events) = events.try_borrow_mut() else {
            return false;
        };
        if !(events.system_keys && events.guest_focus) {
            return false;
        }
        events.keyboard(key.key, key.state, key.repeat, false, None);
        events.repaint();
        true
    })
}

fn shortcuts_help(macos: bool) -> String {
    let mut help =
        String::from("Enter / double-click: start the selected machine (on an image: Start…)\n");
    for (shortcut, action) in [
        ("Ctrl+Alt+P", "pause / resume"),
        ("Ctrl+Alt+T", "hold to fast-forward (Unlimited)"),
        ("Ctrl+Alt+Q", "stop and return to library"),
        (
            "Ctrl+Alt+S",
            "Save state (a machine opened with Open…, when its core saves state)",
        ),
        ("Ctrl+Alt+L", "Load state (the state Save state wrote)"),
        (
            "Ctrl+Alt+End",
            "send guest Ctrl+Alt+Del; reset a halted machine",
        ),
        ("Ctrl+Alt+F", "next A: image (Shift for B:)"),
        ("Ctrl+Alt+E", "eject A: (Shift for B:)"),
        ("Ctrl+Alt+I", "Insert… into A: (Shift for B:)"),
        ("Ctrl+Alt+M", "toggle user mute (also while speed-muted)"),
        ("Ctrl+Alt+G", "release pointer"),
        (
            "Ctrl+Alt+Shift+D",
            "Save machine report (Tools › Diagnostics)",
        ),
        ("Ctrl+Alt+Enter", "full screen"),
        ("Ctrl+N", "New machine… (makes the image and a machine)"),
        ("Ctrl+Shift+N", "New machine from image…"),
        ("F2", "Rename… (machine)"),
        ("F5", "Refresh remote (the registry the library shows)"),
        ("Ctrl+D", "Duplicate… (machine)"),
        ("Delete", "Delete machine… / Delete tag… (image)"),
        ("Ctrl+O", "Open… a file, with whichever core runs it"),
        (
            "Ctrl+,",
            "Settings… (the selected machine; images use Image › Settings…)",
        ),
    ] {
        help.push_str(&format!(
            "{}: {action}\n",
            shell_model::shortcut_text(shortcut, macos)
        ));
    }
    if macos {
        help.push_str("⌘⇧/: keyboard shortcuts\nPress ⌘ first for Command+Shift shortcuts. Shift pressed first remains guest input.\n");
        help.push_str("⌘Q: quit; ⌘W: close; ⌘H: hide; ⌘M: minimize; ⌘Tab: switch application\nCommand chords never reach the guest. Ctrl and Alt remain guest input.\n");
        help.push_str("macOS takes Ctrl+arrows (Mission Control, Spaces), Ctrl+digits, Ctrl+F1–F8, Ctrl+Space and F11 before the guest sees them. Turn on Preferences › General › Send macOS system shortcuts to the guest (needs Accessibility access), or turn those shortcuts off in System Settings › Keyboard › Keyboard Shortcuts › Mission Control.\n");
    } else {
        help.push_str("F1: keyboard shortcuts (outside guest focus)\nCtrl+N/O/, also require shell focus while running.\n");
    }
    help.push_str("The library keys (F2, Duplicate, Delete, New machine from image) work only while no machine runs; while a machine has the keyboard, Ctrl, Alt and Delete reach the guest.\n");
    help.push_str("\nClick the emulated screen to focus guest input and capture the pointer.\nMenus, dialogs and focus loss release the pointer and held guest keys.");
    help
}

/// The display card a launched image shows. The text-only VGA and the
/// register-level VGA share the VGA role, so both are found by role rather
/// than by device model, as for machine files.
fn display_device(devices: &[assembly::DeviceConfig]) -> Option<assembly::DeviceId> {
    devices
        .iter()
        .map(|d| d.id)
        .find(|id| matches!(id, assembly::DeviceId::Mda | assembly::DeviceId::Vga))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_launched_image_finds_its_register_level_vga() {
        use assembly::{DeviceConfig, DeviceId, DeviceKind, VgaCfg};
        let register_vga = vec![DeviceConfig::new(
            DeviceId::Vga,
            DeviceKind::VgaRegisters(VgaCfg {}),
            Vec::new(),
        )];
        assert_eq!(display_device(&register_vga), Some(DeviceId::Vga));
        assert_eq!(display_device(&[]), None);
    }

    use super::*;
    use std::collections::BTreeMap;
    use ElementState::{Pressed, Released};

    fn shell() -> Shell {
        let mut rom = vec![0xf4; 32768];
        rom[..3].copy_from_slice(&[0xfa, 0xeb, 0xfe]);
        rom[0x7ff0..0x7ff5].copy_from_slice(&[0xea, 0, 0, 0, 0xf8]);
        let config = assembly::bios(rom, None);
        let presenter = Presenter::new(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap();
        Shell {
            args: crate::parse_args([]).unwrap(),
            library: Vec::new(),
            image_library: crate::library_view::LibraryView::new(None),
            retain_workspace_on_stop: false,
            record_exceptions: false,
            exit_requested: false,
            store_path_input: String::new(),
            selected: None,
            presenter: Some(presenter),
            paused: false,
            classic: true,
            toolbar: true,
            statusbar: true,
            fullscreen: false,
            integer_scale: false,
            dialog: None,
            error_report: None,
            message: String::new(),
            texture: None,
            logo: None,
            events: Rc::default(),
            errors: Rc::default(),
            window: None,
            screen: egui::Rect::NOTHING,
            guest_focus: true,
            pointer: egui::Pos2::ZERO,
            modifiers: [false; 8],
            macos_shortcuts: false,
            host_modifiers: ModifiersState::empty(),
            fast_forward_key: None,
            consumed: BTreeSet::new(),
            held: BTreeSet::new(),
            guest_held: BTreeSet::new(),
            session: 0,
            modal_paused: false,
            hidden: false,
            minimized: false,
            open_path: String::new(),
            preferences: Default::default(),
            preferences_draft: Default::default(),
            preferences_path: None,
            preferences_path_error: None,
            rom_folder_input: String::new(),
            library_revision: 0,
            readiness: None,
            prepared: Default::default(),
            discovery_session: Default::default(),
            discovery_roots: Vec::new(),
            start_request: None,
            checked_selection: None,
            insert_dialog: None,
            picker: None,
            pending_media: None,
            blank_disk: None,
            settings: None,
            preferences_dialog: None,
            credential_store: Arc::new(image_registry::credential_store::SessionStore::default()),
            discovery_warnings: BTreeSet::new(),
            discovery_control: Default::default(),
            firmware_cache: None,
            default_store_root: None,
            default_firmware_roots: Vec::new(),
            rescan: false,
            refresh_listing: false,
            discovery_finished: false,
            stop_prompt: None,
            stop_prompt_input: (false, false),
            restore_input: None,
            transfer_prompt: None,
            system_keys: crate::system_keys::fake::system_keys().0,
            clipboard: Box::<ui_kit::text_clipboard::fake::FakeClipboard>::default(),
            file_dialogs: file_dialog::FileDialogs::with_host(
                file_dialog::fake::Host::default().boxed(),
            ),
            creating: None,
            cores: Arc::new(core_launch::Cores::new(platforms::cores())),
            host_dirs: Err("no host directories in tests".into()),
            player: None,
            player_commands: Vec::new(),
            opening: None,
            opened: None,
            open_refusal: None,
            player_log: Vec::new(),
        }
    }

    #[test]
    fn settings_save_selects_the_child_and_live_sessions_reject_publication() {
        let temp = machine_file::test_support::TempDir::new("shell-settings");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let parent = temp.path().join("machines/example.toml");
        std::fs::write(
            &parent,
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n",
        )
        .unwrap();
        let mut shell = shell();
        shell.library.push(LibraryEntry::pending(parent));
        shell.selected = Some(0);
        shell.command(Command::Settings, &egui::Context::default());
        assert!(shell.settings.as_ref().unwrap().read_only);
        assert!(!shell.guest_focus);
        shell.stop();
        shell.open_settings();
        shell.settings.as_mut().unwrap().set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(1024),
        );
        let child = shell.settings.as_ref().unwrap().target.clone();
        let alias = child
            .parent()
            .unwrap()
            .join("../local")
            .join(child.file_name().unwrap());
        shell.library.push(LibraryEntry::pending(alias.clone()));
        shell.save_settings(false);
        assert_eq!(shell.library.len(), 2);
        assert_eq!(shell.library[shell.selected.unwrap()].path, alias);
        assert!(shell.settings.is_some());
        assert!(child.exists());
        shell.settings.as_mut().unwrap().set(
            "memory.extended_kib",
            assembly::capabilities::Value::Integer(2048),
        );
        shell.settings.as_mut().unwrap().overwrite = true;
        let mut live = self::shell();
        shell.presenter = live.presenter.take();
        let before = std::fs::read(&child).unwrap();
        shell.save_settings(true);
        assert_eq!(std::fs::read(&child).unwrap(), before);
        assert!(shell.settings.is_some());
    }

    fn take_guest_scancodes(shell: &mut Shell) -> Vec<u8> {
        let presenter = shell.presenter.as_mut().unwrap();
        let mut codes = Vec::new();
        for _ in 0..32 {
            codes.extend(presenter.machine_mut().deliver_keys_for_test());
            if presenter
                .assembled()
                .machine
                .bus_mut()
                .port_read(0x64, 1)
                .unwrap()
                & 1
                != 0
            {
                presenter
                    .assembled()
                    .machine
                    .bus_mut()
                    .port_read(0x60, 1)
                    .unwrap();
            }
            if presenter.keyboard().pending_events() == 0 {
                return codes;
            }
            let cap = presenter.assembled().machine.virtual_time() + 100_000;
            presenter
                .assembled()
                .machine
                .run_bounded(Some(cap))
                .unwrap();
        }
        panic!("keyboard queue did not drain");
    }

    #[test]
    fn preferences_opens_the_registries_page_with_the_shell_credential_store() {
        let mut shell = shell();
        shell.command(Command::Preferences, &egui::Context::default());
        assert!(shell.has_dialog());
        let dialog = shell.preferences_dialog.as_ref().unwrap();
        assert_eq!(dialog.tab, crate::preferences_sheet::Tab::Registries);
        assert_eq!(
            dialog.registries.store_kind(),
            image_registry::credential_store::StoreKind::Session
        );
    }

    #[test]
    fn image_push_and_pull_open_with_the_shell_accounts_only_while_no_machine_runs() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.command(Command::Push, &ctx);
        shell.command(Command::PullReference, &ctx);
        assert!(shell.image_library.push.is_none() && shell.image_library.pull.is_none());
        shell.stop();
        let temp = machine_file::test_support::TempDir::new("shell-registry-transfer");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, "default_registry = \"nas.example:5000\"\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n").unwrap();
        shell.preferences_path = Some(path);
        shell.command(Command::PullReference, &ctx);
        let pull = shell.image_library.pull.as_ref().unwrap();
        assert_eq!(pull.default_host, "nas.example:5000");
        assert!(shell.has_dialog());
        shell.image_library.pull = None;
        // Push to… takes a selected library item; without one it opens
        // nothing.
        shell.command(Command::Push, &ctx);
        assert!(shell.image_library.push.is_none());
        shell.image_library.selected = Some(library_model::listing::ImageRow {
            reference: "test:source".into(),
            aliases: Vec::new(),
            descriptor: image_store::Descriptor::new(
                image_store::media_type::MANIFEST,
                image_store::Digest::of(b"image"),
                1,
            ),
            summary: None,
            failure: None,
        });
        // Without a store worker the dialog ends at once instead of waiting.
        shell.command(Command::Push, &ctx);
        let push = shell.image_library.push.as_ref().unwrap();
        assert_eq!(push.default_host, "nas.example:5000");
        assert!(push
            .error
            .as_deref()
            .is_some_and(|e| e.contains("worker stopped")));
        assert!(matches!(
            push.state,
            crate::registry_transfer::PushState::Ready
        ));
    }

    /// The toolbar's Push… sits beside Pull… with the Push icon: Push to… of
    /// the selected library items, or Push here… while a Remotes registry
    /// or namespace is shown. Image › Push to… and Image › Push here… follow
    /// the same selection. Without one both are disabled, say why, and are
    /// refused in code too; Push here… waits while its registry does not
    /// answer.
    #[test]
    fn push_to_and_push_here_follow_the_selection_and_are_refused_without_one() {
        use crate::library_view::remotes::Node;
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.stop();
        assert!(!shell.command_allowed(Command::Push));
        assert_eq!(
            shell.disabled_reason(Command::Push),
            Some("Select images, installation media or firmware packages in the library")
        );
        assert!(!shell.command_allowed(Command::PushHere));
        assert_eq!(
            shell.disabled_reason(Command::PushHere),
            Some("Select a registry or namespace in Remotes")
        );
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let push = label_rect(&texts, "Push…").center();
        assert!(click_toolbar(&ctx, &mut shell, push).is_empty());
        // Refused in code, not only greyed.
        shell.command(Command::Push, &ctx);
        shell.command(Command::PushHere, &ctx);
        let accounts = shell.registry_accounts();
        shell.image_library.begin_push(accounts);
        assert!(shell.image_library.push.is_none() && shell.image_library.push_batch.is_none());

        // A selected image: the toolbar's Push… is Push to….
        shell.image_library.selected = Some(library_model::listing::ImageRow {
            reference: "test:source".into(),
            aliases: Vec::new(),
            descriptor: image_store::Descriptor::new(
                image_store::media_type::MANIFEST,
                image_store::Digest::of(b"image"),
                1,
            ),
            summary: None,
            failure: None,
        });
        assert!(shell.command_allowed(Command::Push));
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let push = label_rect(&texts, "Push…").center();
        assert_eq!(click_toolbar(&ctx, &mut shell, push), [Command::Push]);
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Image").center());
        let drawn = draw(&ctx, &mut shell, Vec::new());
        assert!(drawn.text.iter().any(|(t, _)| t == "Push to…"));
        assert!(drawn.text.iter().any(|(t, _)| t == "Push here…"));
        assert_eq!(
            click(
                &ctx,
                &mut shell,
                label_rect(&drawn.text, "Push to…").center()
            ),
            [Command::Push]
        );
        // Several rows selected: the row selected alone behind them takes no
        // command of one item.
        shell.image_library.library_multi = vec!["a:1".into(), "b:1".into()];
        for command in [
            Command::Start,
            Command::DeleteTag,
            Command::DeleteSelection,
            Command::Settings,
            Command::Layers,
            Command::NewFromImage,
            Command::ImageSettings,
            Command::ImageGaming,
        ] {
            assert!(!shell.command_allowed(command), "{command:?}");
            assert_eq!(
                shell.disabled_reason(command),
                Some("Several items are selected: select one"),
                "{command:?}"
            );
        }
        shell.command(Command::DeleteTag, &ctx);
        assert!(!shell.image_library.has_dialog());
        shell.image_library.library_multi.clear();

        // A Remotes namespace: the toolbar's Push… is Push here….
        let accounts = shell.registry_accounts();
        shell.image_library.attach_remotes(accounts);
        shell.image_library.remote.node = Some(Node {
            host: "registry.unemu.dev".into(),
            namespace: Some(Some("unemu".into())),
            group: None,
        });
        assert!(shell.command_allowed(Command::PushHere));
        assert!(!shell.command_allowed(Command::Push));
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let push = label_rect(&texts, "Push…").center();
        assert_eq!(click_toolbar(&ctx, &mut shell, push), [Command::PushHere]);
        shell.command(Command::PushHere, &ctx);
        let batch = shell.image_library.push_batch.as_ref().unwrap();
        assert_eq!(
            batch.here().map(|p| p.prefix()).as_deref(),
            Some("registry.unemu.dev/unemu/")
        );
        shell.image_library.push_batch = None;

        // A registry that does not answer: Push here… waits.
        let model = shell.image_library.remote.model.as_mut().unwrap();
        model.touch();
        model.registries[0].state = crate::remote_browse::State::Unreachable {
            reason: "timed out".into(),
            stale: false,
        };
        assert!(!shell.command_allowed(Command::PushHere));
        assert_eq!(
            shell.disabled_reason(Command::PushHere),
            Some("The registry does not answer; Push here… waits until Try again reaches it")
        );
        shell.command(Command::PushHere, &ctx);
        assert!(shell.image_library.push_batch.is_none());
    }

    /// The toolbar has the remotes mock-up's Refresh and Registries…, and
    /// View has Refresh remote with F5. While the library shows a registry's
    /// node, Refresh and Refresh remote list it again; while it shows a
    /// collection they are disabled. Registries… opens Preferences.
    #[test]
    fn refresh_remote_is_on_the_toolbar_and_in_view_while_a_registry_shows() {
        use crate::library_view::remotes::Node;
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.stop();
        // Shown but disabled while the library shows a collection.
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let refresh = label_rect(&texts, "Refresh").center();
        assert!(click_toolbar(&ctx, &mut shell, refresh).is_empty());
        assert!(!shell.command_allowed(Command::RefreshRemote));
        // Registries… opens Preferences on its Registries page.
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let registries = label_rect(&texts, "Registries…").center();
        assert_eq!(
            click_toolbar(&ctx, &mut shell, registries),
            [Command::Preferences]
        );
        shell.image_library.remote.node = Some(Node {
            host: "registry.unemu.dev".into(),
            namespace: None,
            group: None,
        });
        assert!(shell.command_allowed(Command::RefreshRemote));
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        let commands = click_toolbar(&ctx, &mut shell, label_rect(&texts, "Refresh").center());
        assert_eq!(commands, [Command::RefreshRemote]);
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "View").center());
        let drawn = draw(&ctx, &mut shell, Vec::new());
        let item = label_rect(&drawn.text, "Refresh remote");
        let key = label_rect(&drawn.text, "F5");
        assert!(
            (item.center().y - key.center().y).abs() < 2.0,
            "{item:?} {key:?}"
        );
        let commands = click(&ctx, &mut shell, item.center());
        assert_eq!(commands, [Command::RefreshRemote]);
    }

    /// The toolbar's Pull… and Image › Pull… take the tags selected in
    /// Remotes: without one they are disabled and say why. The toolbar shows
    /// Pull… while the library shows Remotes, in the room of the speed
    /// control, which applies to a running machine only. Image › Pull by
    /// reference… opens the reference dialog.
    #[test]
    fn pull_takes_the_remote_selection_and_pull_by_reference_opens_its_dialog() {
        use crate::library_view::remotes::Node;
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.stop();
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        assert!(!shows(&texts, "Pull…") && shows(&texts, "Speed:"));
        shell.image_library.remote.node = Some(Node {
            host: "registry.unemu.dev".into(),
            namespace: None,
            group: None,
        });
        let (texts, _) = toolbar_pass(&ctx, &mut shell, Vec::new());
        assert!(!shows(&texts, "Speed:"));
        let registries = label_rect(&texts, "Registries…");
        let pull = label_rect(&texts, "Pull…");
        assert!(
            (pull.center().y - registries.center().y).abs() < 4.0 && registries.right() <= 1040.0,
            "one line at the default width: {pull:?} {registries:?}"
        );
        let pull = pull.center();
        assert!(click_toolbar(&ctx, &mut shell, pull).is_empty());
        assert!(!shell.command_allowed(Command::Pull));
        assert_eq!(
            shell.disabled_reason(Command::Pull),
            Some("Select a tag in Remotes to pull")
        );
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Image").center());
        let drawn = draw(&ctx, &mut shell, Vec::new());
        assert!(drawn.text.iter().filter(|(t, _)| t == "Pull…").count() >= 2);
        // Image › Start… and Image › Pull… both name Enter.
        assert_eq!(drawn.text.iter().filter(|(t, _)| t == "Enter").count(), 2);
        let item = label_rect(&drawn.text, "Pull by reference…");
        assert_eq!(
            click(&ctx, &mut shell, item.center()),
            [Command::PullReference]
        );
    }

    /// File › Get templates… shows Remotes › the default registry › unemu/
    /// › Machine images with the shell's accounts, and only while no machine
    /// runs; the machine file selection goes. Without a store worker the
    /// listing ends at once with the reason.
    #[test]
    fn get_templates_selects_the_default_registry_templates_only_while_no_machine_runs() {
        use crate::{library_view::remotes::Group, remote_browse::State};
        let mut shell = shell();
        let ctx = egui::Context::default();
        assert!(!shell.command_allowed(Command::GetTemplates));
        shell.command(Command::GetTemplates, &ctx);
        assert!(!shell.image_library.remote_shown());
        shell.stop();
        assert!(shell.command_allowed(Command::GetTemplates));
        let temp = machine_file::test_support::TempDir::new("shell-templates");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, "default_registry = \"nas.example:5000\"\n").unwrap();
        shell.preferences_path = Some(path);
        shell.selected = Some(0);
        shell.command(Command::GetTemplates, &ctx);
        assert_eq!(shell.selected, None);
        let node = shell.image_library.remote.node.clone().unwrap();
        assert_eq!(node.host, "nas.example:5000");
        assert_eq!(node.namespace, Some(Some("unemu".into())));
        assert_eq!(node.group, Some(Group::Machines));
        let registry = shell
            .image_library
            .remote
            .model
            .as_ref()
            .unwrap()
            .registry("nas.example:5000")
            .unwrap();
        assert!(
            matches!(&registry.state, State::Failed(message) if message.contains("worker stopped")),
            "{:?}",
            registry.state
        );
        assert!(!shell.has_dialog(), "no dialog opens");
    }

    /// Log in… of a registry in Remotes opens Preferences › Registries on
    /// that registry's Log in dialog, never on another registry's row; a
    /// registry without an entry is the next test's.
    #[test]
    fn log_in_from_remotes_opens_that_registry_login() {
        let mut shell = shell();
        shell.stop();
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("shell-remotes-login");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            "default_registry = \"nas.example:5000\"\n[[registries]]\nhost = \"lab.example\"\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n",
        )
        .unwrap();
        shell.preferences_path = Some(path);
        // Log in… in the Remotes list asks for this registry's login.
        shell.image_library.login_wanted = Some("nas.example:5000".into());
        // The frame turns the request into the Preferences command.
        let mut commands = Vec::new();
        shell.library_commands(&mut commands);
        assert_eq!(commands, [Command::Preferences]);
        shell.command(Command::Preferences, &ctx);
        assert!(shell.image_library.login_wanted.is_none());
        let page = &shell.preferences_dialog.as_ref().unwrap().registries;
        assert_eq!(page.selected_row().unwrap().entry.host, "nas.example:5000");
        assert_eq!(page.login.as_ref().unwrap().host, "nas.example:5000");
        shell.close_preferences();
        assert!(shell.preferences_dialog.is_none());
        // Preferences from the menu opens no Log in dialog.
        shell.command(Command::Preferences, &ctx);
        assert!(shell
            .preferences_dialog
            .as_ref()
            .unwrap()
            .registries
            .login
            .is_none());
    }

    #[test]
    fn log_in_for_a_registry_without_an_entry_opens_add_registry_and_selects_no_other_row() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("shell-templates-add");
        let path = temp.path().join("preferences.toml");
        // The default registry has no [[registries]] entry, as UNEMU_REGISTRY
        // or a bare default_registry allows.
        std::fs::write(&path, "default_registry = \"nas.example:5000\"\n").unwrap();
        shell.preferences_path = Some(path);
        shell.image_library.login_wanted = Some("nas.example:5000".into());
        let mut commands = Vec::new();
        shell.library_commands(&mut commands);
        assert_eq!(commands, [Command::Preferences]);
        shell.command(Command::Preferences, &ctx);
        let page = &shell.preferences_dialog.as_ref().unwrap().registries;
        assert!(page
            .rows
            .iter()
            .all(|row| row.entry.host != "nas.example:5000"));
        let editor = page.editor.as_ref().expect("Add registry… is open");
        assert_eq!(editor.original, None);
        assert_eq!(editor.host, "nas.example:5000");
        // Neither the built-in registry nor any other row is selected, so no
        // login can go to another registry by mistake.
        assert!(page.selected_row().is_none());
        assert!(page.login.is_none() && !page.can_log_in());
    }

    /// One frame of the shell with Preferences open: the logic step, then
    /// the dialog as the shell draws it.
    fn preferences_frame(shell: &mut Shell, frame: &mut eframe::Frame, ui: &mut egui::Ui) {
        eframe::App::logic(shell, ui.ctx(), frame);
        if let Some(dialog) = shell.preferences_dialog.as_mut() {
            let _ = crate::preferences_sheet::show(ui.ctx(), dialog, true);
        }
    }

    /// The registry Log in dialog takes a pasted user name and password with
    /// Command+V on macOS and Ctrl+V elsewhere, gives nothing of the password
    /// to copy or cut, and Enter right after a paste logs in with the pasted
    /// password, against a registry on this computer.
    #[test]
    fn the_registry_login_fields_take_a_paste_and_the_password_never_leaves() {
        use image_registry::test_support::{MockRegistry, Policy};
        use ui_kit::text_clipboard::fake::{press, Host, CTRL_COMMAND, MAC_COMMAND};
        fn page(shell: &mut Shell) -> &mut crate::registries_model::RegistriesPage {
            &mut shell.preferences_dialog.as_mut().unwrap().registries
        }
        for modifiers in [MAC_COMMAND, CTRL_COMMAND] {
            let registry = MockRegistry::start(Policy {
                user: Some(("ci".into(), "synthetic-secret".into())),
                ..Policy::default()
            });
            let host_name = registry.host();
            let mut shell = shell();
            shell.stop();
            let temp = machine_file::test_support::TempDir::new("shell-login-paste");
            let path = temp.path().join("preferences.toml");
            std::fs::write(
                &path,
                format!(
                    "[[registries]]\nhost = \"{host_name}\"\nprivate = true\nplain_http = true\nuser = \"old-user\"\n"
                ),
            )
            .unwrap();
            shell.preferences_path = Some(path);
            shell.image_library.login_wanted = Some(host_name.clone());
            let mut host = Host::new("ci");
            shell.clipboard = Box::new(host.clipboard.clone());
            shell.command(Command::Preferences, &host.ctx);
            let mut frame = eframe::Frame::_new_kittest();
            let login = |shell: &mut Shell| {
                let dialog = page(shell).login.as_ref().expect("Log in is open");
                (dialog.user.clone(), dialog.password.clone())
            };

            host.pass(Vec::new(), |ui| {
                preferences_frame(&mut shell, &mut frame, ui)
            });
            let painted = host.pass(Vec::new(), |ui| {
                preferences_frame(&mut shell, &mut frame, ui)
            });
            // The dialog draws above the registry table, which also names
            // the user.
            let (_, user) = painted
                .iter()
                .rev()
                .find(|(text, _)| text == "old-user")
                .expect("the user field shows its text");
            host.click(user.center(), |ui| {
                preferences_frame(&mut shell, &mut frame, ui)
            });
            host.pass(
                vec![
                    press(egui::Key::A, modifiers),
                    press(egui::Key::V, modifiers),
                ],
                |ui| preferences_frame(&mut shell, &mut frame, ui),
            );
            assert_eq!(login(&mut shell), ("ci".into(), String::new()));

            host.set_text("wrong-secret");
            host.ctx.memory_mut(|memory| {
                memory.request_focus(crate::preferences_sheet::password_field_id())
            });
            host.pass(vec![press(egui::Key::V, modifiers)], |ui| {
                preferences_frame(&mut shell, &mut frame, ui)
            });
            assert_eq!(login(&mut shell), ("ci".into(), "wrong-secret".into()));
            assert_eq!(host.reads(), 2, "one clipboard read per paste");

            host.set_text("unchanged");
            for key in [egui::Key::A, egui::Key::C, egui::Key::X] {
                host.pass(vec![press(key, modifiers)], |ui| {
                    preferences_frame(&mut shell, &mut frame, ui)
                });
            }
            assert_eq!(login(&mut shell).1, "wrong-secret");
            assert_eq!((host.text().as_str(), host.writes()), ("unchanged", 0));

            // The password is still selected: paste over it and press Enter
            // in the same pass.
            host.set_text("synthetic-secret");
            host.pass(
                vec![
                    press(egui::Key::V, modifiers),
                    press(egui::Key::Enter, egui::Modifiers::NONE),
                ],
                |ui| preferences_frame(&mut shell, &mut frame, ui),
            );
            page(&mut shell).wait();
            assert!(page(&mut shell).login.is_none(), "the login succeeded");
            assert_eq!(page(&mut shell).signed_in_count(), 1);
        }
    }

    #[test]
    fn quitting_during_a_push_asks_first_and_cancels_only_on_yes() {
        let mut lab = crate::registry_transfer::tests::Lab::new(true);
        let cancel = lab.held_push();
        let mut shell = shell();
        shell.stop();
        shell.image_library =
            std::mem::replace(&mut lab.view, crate::library_view::LibraryView::new(None));
        let ctx = egui::Context::default();
        shell.request_exit(&ctx);
        assert_eq!(shell.transfer_prompt, Some("push"));
        assert!(shell.has_dialog() && !shell.exit_requested);
        assert!(matches!(
            shell.exit_progress(),
            crate::app_menu::ExitProgress::Asking
        ));
        assert!(!cancel.load(std::sync::atomic::Ordering::SeqCst));
        shell.answer_transfer_prompt(false, &ctx);
        assert!(shell.transfer_prompt.is_none() && !shell.exit_requested);
        assert!(shell.image_library.running_transfer().is_some());
        assert!(!cancel.load(std::sync::atomic::Ordering::SeqCst));

        shell.request_exit(&ctx);
        assert_eq!(shell.transfer_prompt, Some("push"));
        lab.registry.release();
        shell.answer_transfer_prompt(true, &ctx);
        assert!(cancel.load(std::sync::atomic::Ordering::SeqCst));
        assert!(shell.image_library.push.is_none());
        assert!(shell.exit_requested);
    }

    fn keyboard_shell() -> Shell {
        let mut shell = shell();
        *shell.presenter.as_mut().unwrap().assembled() =
            machine_file::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        shell.sync_routing();
        shell
    }

    fn media_shell() -> Shell {
        let mut shell = shell();
        let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: ["first.img", "second.img"]
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
        shell.presenter = Some(
            Presenter::new(
                &config,
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
                &mut io::sink(),
            )
            .unwrap(),
        );
        shell
    }

    /// A shell whose file dialogs are scripted by the returned host.
    fn file_dialog_shell() -> (Shell, file_dialog::fake::Host) {
        let mut shell = media_shell();
        let host = file_dialog::fake::Host::default();
        shell.file_dialogs = file_dialog::FileDialogs::with_host(host.boxed());
        (shell, host)
    }

    fn floppy_image(temp: &machine_file::test_support::TempDir, name: &str) -> std::path::PathBuf {
        let path = temp.path().join(name);
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        path.canonicalize().unwrap()
    }

    /// Each file dialog keeps the kind, title, filter, start folder and file
    /// name the synchronous dialog at its call site had.
    #[test]
    fn every_file_dialog_opens_with_its_former_title_filter_and_folder() {
        use crate::library_view::{firmware::SourceTarget, install_media::Pick};
        use file_dialog::Kind;
        use std::path::Path;
        type Filters = &'static [(&'static str, &'static [&'static str])];
        /// Kind, title, filters, start folder and file name, as the call
        /// site's synchronous dialog had them.
        type Former<'a> = (Kind, &'a str, Filters, Option<&'a Path>, Option<&'a str>);
        const DISK_IMAGES: Filters = &[("Disk images", &["img", "ima", "dsk"])];
        const SOURCE: &str = "Choose the source archive (zip or tar)";
        let check = |shell: &Shell,
                     asker: FileFor,
                     (kind, title, filters, directory, file_name): Former| {
            let request = shell.file_request(&asker).unwrap();
            let former: Vec<(String, Vec<String>)> = filters
                .iter()
                .map(|(name, extensions)| {
                    let extensions = extensions.iter().map(|&e| e.to_owned()).collect();
                    ((*name).to_owned(), extensions)
                })
                .collect();
            assert_eq!(request.kind, kind, "{asker:?}");
            assert_eq!(request.title, title, "{asker:?}");
            assert_eq!(request.filters, former, "{asker:?}");
            assert_eq!(request.directory.as_deref(), directory, "{asker:?}");
            assert_eq!(request.file_name.as_deref(), file_name, "{asker:?}");
        };
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("file-dialog-requests");
        let base = floppy_image(&temp, "extra.img");
        let folder = base.parent().unwrap().to_path_buf();
        let folder_name = folder.file_name().unwrap().to_str().unwrap().to_owned();
        let (mut shell, _host) = file_dialog_shell();
        let table: [(FileFor, Former); 9] = [
            (
                FileFor::InsertImage(1),
                (Kind::File, "Choose a floppy image", DISK_IMAGES, None, None),
            ),
            (
                FileFor::DriveImage(0, DriveOwner::Session(0)),
                (Kind::File, "Choose a floppy image", DISK_IMAGES, None, None),
            ),
            (
                FileFor::RomImport,
                (Kind::Folder, "Import ROM folder", &[], None, None),
            ),
            (
                FileFor::FirmwareSource(
                    SourceTarget::Import("set".into(), "/roms/set".into()),
                    None,
                ),
                (Kind::File, SOURCE, &[], Some(Path::new("/roms/set")), None),
            ),
            (
                FileFor::FirmwareSource(SourceTarget::Edit, Some("package".into())),
                (Kind::File, SOURCE, &[], None, None),
            ),
            (
                FileFor::InstallMedia(Pick::Files),
                (Kind::Files, "Choose installation images", &[], None, None),
            ),
            (
                FileFor::InstallMedia(Pick::Folder),
                (
                    Kind::Folder,
                    "Choose a folder of installation images",
                    &[],
                    None,
                    None,
                ),
            ),
            (
                FileFor::InstallMedia(Pick::Extras),
                (
                    Kind::Files,
                    "Choose extras to store with the set",
                    &[],
                    None,
                    None,
                ),
            ),
            (
                FileFor::RomFolder,
                (Kind::Folder, "Locate firmware folder", &[], None, None),
            ),
        ];
        for (asker, former) in table {
            check(&shell, asker, former);
        }
        // The journal's Save as… opens where the typed path points, relative
        // to the base image's folder; with nothing typed, in that folder.
        // Proposing the folder's own name as the file name there is a quirk
        // the synchronous dialog had; it is kept as it was.
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        draft
            .slot
            .entries
            .push(crate::media_dialog::browse(&base, draft.slot.geometry).unwrap());
        draft.select(Some(draft.slot.entries.len() - 1));
        for (typed, directory, file_name) in [
            ("", folder.clone(), folder_name.as_str()),
            ("journals/extra.upfj", folder.join("journals"), "extra.upfj"),
            ("/elsewhere/extra.upfj", "/elsewhere".into(), "extra.upfj"),
        ] {
            shell.insert_dialog.as_mut().unwrap().journal_path = typed.into();
            let former = (
                Kind::Save,
                "Choose a separate journal",
                &[] as Filters,
                Some(directory.as_path()),
                Some(file_name),
            );
            check(&shell, FileFor::InsertJournal(0), former);
        }
        // Create blank disk proposes disk.img in its folder until a path is typed.
        for (typed, directory, file_name) in [
            ("", "/machines", "disk.img"),
            ("images/new.img", "/machines/images", "new.img"),
            ("/elsewhere/new.img", "/elsewhere", "new.img"),
        ] {
            shell.blank_disk = Some(crate::media_dialog::BlankDisk {
                directory: "/machines".into(),
                output: typed.into(),
                ..crate::media_dialog::BlankDisk::new(crate::machines_dir())
            });
            let former = (
                Kind::Save,
                "Create blank disk",
                &[] as Filters,
                Some(Path::new(directory)),
                Some(file_name),
            );
            check(&shell, FileFor::BlankDisk, former);
        }
        assert!(shell.stop());
    }

    #[test]
    fn browse_from_a_drive_picker_returns_at_once_and_the_machine_keeps_running() {
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("file-dialog-browse");
        let image = floppy_image(&temp, "extra.img");
        let (mut shell, host) = file_dialog_shell();
        shell.apply_picker(crate::media_picker::Outcome::Browse(0));
        let asked = host.requests.borrow().clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].title, "Choose a floppy image");
        assert!(shell.file_dialogs.is_open());
        assert!(shell.insert_dialog.is_none(), "nothing is chosen yet");
        assert_eq!(shell.status_message(), "Waiting for the file dialog…");
        // The open file dialog is no shell dialog: frames keep coming and
        // the machine keeps running.
        assert!(!shell.has_dialog());
        shell.tick(&ctx, false, std::time::Duration::from_millis(10));
        assert!(!shell.modal_paused);
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(
            presenter.occluded_since.is_none(),
            "no pause, unlike a shell dialog"
        );
        shell.poll_file_dialog(&ctx);
        assert!(shell.file_dialogs.is_open(), "the user has not answered");
        host.answer(Some(vec![image.clone()]));
        shell.poll_file_dialog(&ctx);
        assert!(!shell.file_dialogs.is_open());
        assert_ne!(shell.status_message(), "Waiting for the file dialog…");
        // The chosen file's own Insert floppy step, as before.
        let draft = shell.insert_dialog.as_ref().unwrap();
        assert_eq!(draft.unit, 0);
        assert!(draft.browsed_only);
        let entry = &draft.slot.entries[draft.selected.unwrap()];
        assert_eq!(entry.medium.image, assembly::ImageSource::File(image));
        assert!(shell.stop());
    }

    #[test]
    fn a_second_file_dialog_is_refused_while_one_is_open() {
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("file-dialog-busy");
        let image = floppy_image(&temp, "extra.img");
        let (mut shell, host) = file_dialog_shell();
        shell.apply_picker(crate::media_picker::Outcome::Browse(0));
        assert_eq!(shell.status_message(), "Waiting for the file dialog…");
        shell.pick_media(crate::library_view::install_media::Pick::Files);
        assert_eq!(
            shell.status_message(),
            "Choose installation images did not open: another file dialog was open"
        );
        // The source archive picker that Edit metadata… opens by itself once
        // the package is read takes the same way, and says so too.
        shell.image_library.firmware_source_pick =
            Some(crate::library_view::firmware::SourceTarget::Edit);
        let target = shell.image_library.firmware_source_pick.take().unwrap();
        shell.pick_firmware_source(target);
        assert_eq!(
            shell.status_message(),
            "Choose the source archive (zip or tar) did not open: another file dialog was open"
        );
        assert_eq!(
            host.requests.borrow().len(),
            1,
            "only the first dialog opens"
        );
        host.answer(Some(vec![image]));
        shell.poll_file_dialog(&ctx);
        assert_eq!(shell.insert_dialog.as_ref().map(|d| d.unit), Some(0));
        assert!(!shell.file_dialogs.refused());
        // The next dialog opens once the first has answered.
        shell.insert_dialog = None;
        shell.pick_media(crate::library_view::install_media::Pick::Folder);
        assert_eq!(host.requests.borrow().len(), 2);
        assert_eq!(
            host.requests.borrow()[1].title,
            "Choose a folder of installation images"
        );
        assert!(shell.stop());
    }

    /// The frame's logic step, which runs before anything draws, takes the
    /// answer: the shell needs no other call to receive it.
    #[test]
    fn the_frame_logic_delivers_the_answer_before_drawing() {
        let ctx = egui::Context::default();
        let temp = machine_file::test_support::TempDir::new("file-dialog-frame");
        let image = floppy_image(&temp, "extra.img");
        let (mut shell, host) = file_dialog_shell();
        let mut frame = eframe::Frame::_new_kittest();
        shell.apply_picker(crate::media_picker::Outcome::Browse(0));
        eframe::App::logic(&mut shell, &ctx, &mut frame);
        assert!(shell.file_dialogs.is_open(), "the user has not answered");
        host.answer(Some(vec![image.clone()]));
        eframe::App::logic(&mut shell, &ctx, &mut frame);
        assert!(!shell.file_dialogs.is_open());
        let draft = shell.insert_dialog.as_ref().expect("the answer arrived");
        assert!(draft.browsed_only);
        assert_eq!(
            draft.slot.entries[draft.selected.unwrap()].medium.image,
            assembly::ImageSource::File(image)
        );
        assert!(shell.stop());
    }

    /// Files and library state the delivery rows share.
    struct Delivery {
        temp: machine_file::test_support::TempDir,
        image: std::path::PathBuf,
        second: std::path::PathBuf,
        library: crate::library_test_support::Fixture,
        cache: image_store::summary::SummaryCache,
    }

    impl Delivery {
        fn new() -> Self {
            let temp = machine_file::test_support::TempDir::new("file-dialog-delivery");
            let library = crate::library_test_support::Fixture::new();
            crate::library_view::firmware::tests::publish_open_package(&library);
            let cache = crate::library_view::firmware::tests::warm(&library.store);
            Self {
                image: floppy_image(&temp, "extra.img"),
                second: floppy_image(&temp, "second.img"),
                temp,
                library,
                cache,
            }
        }

        fn folder(&self) -> std::path::PathBuf {
            self.temp.path().canonicalize().unwrap()
        }

        /// The image library on its synthetic worker, over the shared store.
        fn library(&self, shell: &mut Shell) {
            shell.image_library =
                crate::library_view::LibraryView::synthetic(self.library.store.root().to_owned());
        }

        /// Edit metadata… open and read for the store's open package.
        fn edit_package(&self, shell: &mut Shell) {
            self.library(shell);
            shell
                .image_library
                .select_first_package(&self.cache, &mut shell.selected);
            shell.image_library.begin_firmware_edit(false);
            crate::library_view::firmware::tests::poll_until(&mut shell.image_library, |v| {
                v.firmware_edit_source().is_some()
            });
        }
    }

    fn insert_dialog_for(shell: &mut Shell, unit: u8) {
        let slot = shell.floppy_slot(0).unwrap();
        shell.insert_dialog = Some(crate::media_dialog::InsertDialog::new(unit, slot));
    }

    /// Drive A's Insert floppy dialog with the image selected and a
    /// separate journal path typed.
    fn journal_dialog(shell: &mut Shell, env: &Delivery) {
        insert_dialog_for(shell, 0);
        let draft = shell.insert_dialog.as_mut().unwrap();
        let entry = crate::media_dialog::browse(&env.image, draft.slot.geometry).unwrap();
        draft.add_browsed(entry);
        draft.journal = true;
        draft.journal_path = "typed.upfj".into();
    }

    fn has_image(shell: &Shell, image: &std::path::Path) -> bool {
        shell.insert_dialog.as_ref().is_some_and(|draft| {
            draft
                .slot
                .entries
                .iter()
                .any(|entry| entry.medium.image == assembly::ImageSource::File(image.to_path_buf()))
        })
    }

    /// Who still receives a file dialog's answer: asker × what changed while
    /// the dialog was open × answer → effect. Every row starts from a
    /// running machine with drive A's catalogue and a scripted dialog; `ask`
    /// opens the dialog the way its call site does.
    #[test]
    fn a_file_dialog_answer_reaches_only_an_asker_still_there() {
        use crate::library_view::{firmware::SourceTarget, install_media::Pick};
        type Step = fn(&mut Shell, &Delivery);
        type Answer = fn(&Delivery) -> file_dialog::Answer;
        type Effect = fn(&mut Shell, &Delivery, &str);
        type Row = (&'static str, Step, Step, Answer, Effect);
        fn unchanged(_: &mut Shell, _: &Delivery) {}
        fn image(env: &Delivery) -> file_dialog::Answer {
            Some(vec![env.image.clone()])
        }
        fn cancel(_: &Delivery) -> file_dialog::Answer {
            None
        }
        fn folder(env: &Delivery) -> file_dialog::Answer {
            Some(vec![env.folder()])
        }
        fn archive(env: &Delivery) -> file_dialog::Answer {
            Some(vec![env.temp.path().join("source.zip")])
        }
        const MACHINE_CHANGED: &str =
            "The chosen image was not inserted: the machine changed while the file dialog was open";
        const SOURCE_DROPPED: &str =
            "The chosen source archive was not used: Edit metadata closed or shows another package";
        let rows: &[Row] = &[
            (
                "insert image, same drive",
                |shell, _| {
                    insert_dialog_for(shell, 0);
                    shell.ask_file(FileFor::InsertImage(0));
                },
                unchanged,
                image,
                |shell, env, row| assert!(has_image(shell, &env.image), "{row}"),
            ),
            (
                "insert image, dialog now for drive B",
                |shell, _| {
                    insert_dialog_for(shell, 0);
                    shell.ask_file(FileFor::InsertImage(0));
                },
                |shell, _| insert_dialog_for(shell, 1),
                image,
                |shell, env, row| assert!(!has_image(shell, &env.image), "{row}"),
            ),
            (
                "insert image, dialog closed",
                |shell, _| {
                    insert_dialog_for(shell, 0);
                    shell.ask_file(FileFor::InsertImage(0));
                },
                |shell, _| shell.insert_dialog = None,
                image,
                |shell, _, row| {
                    assert!(
                        shell.insert_dialog.is_none() && shell.dialog.is_none(),
                        "{row}"
                    )
                },
            ),
            (
                "journal, same drive",
                |shell, env| {
                    journal_dialog(shell, env);
                    shell.ask_file(FileFor::InsertJournal(0));
                },
                unchanged,
                |env| Some(vec![env.temp.path().join("extra.upfj")]),
                |shell, env, row| {
                    let typed = &shell.insert_dialog.as_ref().unwrap().journal_path;
                    assert_eq!(
                        typed,
                        &env.temp.path().join("extra.upfj").to_string_lossy(),
                        "{row}"
                    );
                },
            ),
            (
                "journal, dialog now for drive B",
                |shell, env| {
                    journal_dialog(shell, env);
                    shell.ask_file(FileFor::InsertJournal(0));
                },
                |shell, _| insert_dialog_for(shell, 1),
                |env| Some(vec![env.temp.path().join("extra.upfj")]),
                |shell, _, row| {
                    assert_eq!(
                        shell.insert_dialog.as_ref().unwrap().journal_path,
                        "",
                        "{row}"
                    )
                },
            ),
            (
                "journal, cancelled",
                |shell, env| {
                    journal_dialog(shell, env);
                    shell.ask_file(FileFor::InsertJournal(0));
                },
                unchanged,
                cancel,
                |shell, _, row| {
                    let typed = &shell.insert_dialog.as_ref().unwrap().journal_path;
                    assert_eq!(typed, "typed.upfj", "{row}");
                },
            ),
            (
                "blank disk, open",
                |shell, _| {
                    shell.blank_disk =
                        Some(crate::media_dialog::BlankDisk::new(crate::machines_dir()));
                    shell.ask_file(FileFor::BlankDisk);
                },
                unchanged,
                |env| Some(vec![env.temp.path().join("new.img")]),
                |shell, env, row| {
                    let output = &shell.blank_disk.as_ref().unwrap().output;
                    assert_eq!(
                        output,
                        &env.temp.path().join("new.img").to_string_lossy(),
                        "{row}"
                    );
                },
            ),
            (
                "ROM folders, open",
                |shell, _| {
                    shell.dialog = Some(("ROM folders", String::new()));
                    shell.ask_file(FileFor::RomFolder);
                },
                unchanged,
                folder,
                |shell, env, row| {
                    assert_eq!(shell.preferences_draft.rom_dirs, [env.folder()], "{row}")
                },
            ),
            (
                "ROM folders, closed",
                |shell, _| {
                    shell.dialog = Some(("ROM folders", String::new()));
                    shell.ask_file(FileFor::RomFolder);
                },
                |shell, _| shell.dialog = None,
                folder,
                |shell, _, row| assert!(shell.preferences_draft.rom_dirs.is_empty(), "{row}"),
            ),
            (
                "ROM folders, replaced by another dialog",
                |shell, _| {
                    shell.dialog = Some(("ROM folders", String::new()));
                    shell.ask_file(FileFor::RomFolder);
                },
                |shell, _| shell.dialog = Some(("Missing firmware", String::new())),
                folder,
                |shell, _, row| assert!(shell.preferences_draft.rom_dirs.is_empty(), "{row}"),
            ),
            (
                "ROM import, still allowed",
                |shell, env| {
                    assert!(shell.stop());
                    env.library(shell);
                    shell.command(Command::ImportFirmware, &egui::Context::default());
                },
                unchanged,
                folder,
                |shell, _, row| {
                    assert!(shell.image_library.busy, "{row}");
                    assert_eq!(
                        shell.image_library.status.as_deref(),
                        Some("Scanning ROM folder…"),
                        "{row}"
                    );
                },
            ),
            (
                "ROM import, library busy meanwhile",
                |shell, env| {
                    assert!(shell.stop());
                    env.library(shell);
                    shell.command(Command::ImportFirmware, &egui::Context::default());
                },
                |shell, _| shell.image_library.busy = true,
                folder,
                |shell, _, row| {
                    assert_eq!(shell.image_library.status, None, "{row}");
                    assert_eq!(
                        shell.message,
                        "ROM folder not imported: the image library cannot import now",
                        "{row}"
                    );
                },
            ),
            (
                "ROM import, machine running meanwhile",
                |shell, env| {
                    env.library(shell);
                    shell.ask_file(FileFor::RomImport);
                },
                unchanged,
                folder,
                |shell, _, row| {
                    assert!(!shell.image_library.busy, "{row}");
                    assert_eq!(
                        shell.message, "ROM folder not imported: stop the machine first",
                        "{row}"
                    );
                },
            ),
            (
                "installation media, every chosen file",
                |shell, env| {
                    env.library(shell);
                    shell.image_library.begin_media_import();
                    shell.pick_media(Pick::Files);
                },
                unchanged,
                |env| Some(vec![env.image.clone(), env.second.clone()]),
                |shell, env, row| {
                    crate::library_view::firmware::tests::poll_until(
                        &mut shell.image_library,
                        |v| v.reviewed_media().is_some_and(|files| !files.is_empty()),
                    );
                    assert_eq!(
                        shell.image_library.reviewed_media(),
                        Some(vec![env.image.clone(), env.second.clone()]),
                        "{row}"
                    );
                },
            ),
            (
                "source archive, same package",
                |shell, env| {
                    env.edit_package(shell);
                    shell.pick_firmware_source(SourceTarget::Edit);
                },
                unchanged,
                archive,
                |shell, env, row| {
                    let source = env.temp.path().join("source.zip");
                    assert_eq!(
                        shell.image_library.firmware_edit_source(),
                        Some(Some(source.as_path())),
                        "{row}"
                    );
                },
            ),
            (
                "source archive, asked for another package",
                |shell, env| {
                    env.edit_package(shell);
                    let other = FileFor::FirmwareSource(SourceTarget::Edit, Some("other:1".into()));
                    shell.ask_file(other);
                },
                unchanged,
                archive,
                |shell, _, row| {
                    assert_eq!(
                        shell.image_library.firmware_edit_source(),
                        Some(None),
                        "{row}"
                    );
                    assert_eq!(shell.message, SOURCE_DROPPED, "{row}");
                },
            ),
            (
                "source archive, Edit metadata closed",
                |shell, env| {
                    env.edit_package(shell);
                    shell.pick_firmware_source(SourceTarget::Edit);
                },
                |shell, _| shell.image_library.close_dialogs(),
                archive,
                |shell, _, row| assert_eq!(shell.message, SOURCE_DROPPED, "{row}"),
            ),
            (
                "drive image, same session",
                |shell, _| shell.apply_picker(crate::media_picker::Outcome::Browse(0)),
                unchanged,
                image,
                |shell, env, row| {
                    assert!(has_image(shell, &env.image), "{row}");
                    assert!(shell.insert_dialog.as_ref().unwrap().browsed_only, "{row}");
                },
            ),
            (
                "drive image, machine stopped meanwhile",
                |shell, _| shell.apply_picker(crate::media_picker::Outcome::Browse(0)),
                |shell, _| assert!(shell.stop()),
                image,
                |shell, _, row| {
                    assert!(shell.insert_dialog.is_none(), "{row}");
                    assert_eq!(shell.message, MACHINE_CHANGED, "{row}");
                },
            ),
            (
                "drive image, another session",
                |shell, _| shell.apply_picker(crate::media_picker::Outcome::Browse(0)),
                |shell, _| shell.session = shell.session.wrapping_add(1),
                image,
                |shell, _, row| {
                    assert!(shell.insert_dialog.is_none(), "{row}");
                    assert_eq!(shell.message, MACHINE_CHANGED, "{row}");
                },
            ),
            (
                "drive image, another dialog open",
                |shell, _| shell.apply_picker(crate::media_picker::Outcome::Browse(0)),
                |shell, _| shell.dialog = Some(("About unemu", String::new())),
                image,
                |shell, _, row| {
                    assert!(shell.insert_dialog.is_none(), "{row}");
                    assert_eq!(
                        shell.dialog.as_ref().map(|(title, _)| *title),
                        Some("About unemu"),
                        "{row}"
                    );
                    assert_eq!(
                        shell.message, "The chosen image was not inserted: another dialog is open",
                        "{row}"
                    );
                },
            ),
            (
                "drive image, cancelled",
                |shell, _| shell.apply_picker(crate::media_picker::Outcome::Browse(0)),
                unchanged,
                cancel,
                |shell, _, row| {
                    assert!(shell.insert_dialog.is_none(), "{row}");
                    assert_eq!(shell.message, "", "{row}");
                },
            ),
        ];
        let env = Delivery::new();
        let ctx = egui::Context::default();
        for (row, ask, meanwhile, answer, effect) in rows {
            let (mut shell, host) = file_dialog_shell();
            ask(&mut shell, &env);
            assert!(shell.file_dialogs.is_open(), "{row}: no dialog opened");
            meanwhile(&mut shell, &env);
            host.answer(answer(&env));
            shell.poll_file_dialog(&ctx);
            assert!(!shell.file_dialogs.is_open(), "{row}");
            effect(&mut shell, &env, row);
            shell.image_library.shutdown();
            if shell.presenter.is_some() {
                assert!(shell.stop(), "{row}");
            }
        }
    }

    #[test]
    fn invalid_browse_paths_keep_the_session_running_and_the_dialog_editable() {
        let ctx = egui::Context::default();
        for failure in [
            "base alias",
            "missing parent",
            "changed pin",
            "journal alias",
        ] {
            let temp = machine_file::test_support::TempDir::new("browse-validation");
            let path = temp.path().join("extra.img");
            std::fs::write(&path, vec![0; 1_474_560]).unwrap();
            let mut shell = media_shell();
            if failure == "journal alias" {
                let mut config = shell.presenter.as_ref().unwrap().media_config.clone();
                let initial = temp.path().join("initial.img");
                std::fs::write(&initial, vec![0; 1_474_560]).unwrap();
                config.drives[0].media.as_mut().unwrap().entries[0].image =
                    assembly::ImageSource::File(initial);
                config.drives[0].media.as_mut().unwrap().entries[0].write =
                    assembly::WriteMode::journal(temp.path().join("occupied.upfj"));
                shell.presenter = Some(
                    Presenter::new(
                        &config,
                        assembly::AssemblyOptions {
                            allow_unlisted_firmware: true,
                        },
                        &mut io::sink(),
                    )
                    .unwrap(),
                );
            }
            shell.command(Command::InsertFile(0), &ctx);
            let draft = shell.insert_dialog.as_mut().unwrap();
            draft
                .slot
                .entries
                .push(crate::media_dialog::browse(&path, draft.slot.geometry).unwrap());
            draft.selected = Some(2);
            draft.journal = failure != "changed pin";
            draft.journal_path = if failure == "base alias" {
                path.clone()
            } else if failure == "journal alias" {
                temp.path().join("occupied.upfj")
            } else {
                temp.path().join("missing/extra.upfj")
            }
            .to_string_lossy()
            .into_owned();
            if failure == "changed pin" {
                std::fs::write(&path, vec![1; 1_474_560]).unwrap();
            }
            shell.confirm_insert();
            assert!(shell.presenter.as_ref().unwrap().running, "{failure}");
            assert!(
                shell
                    .insert_dialog
                    .as_ref()
                    .is_some_and(|draft| draft.error.is_some()),
                "{failure}"
            );
            assert!(shell.dialog.is_none());
            assert!(shell.presenter.as_ref().unwrap().media_events.is_empty());
            assert_eq!(
                shell
                    .presenter
                    .as_mut()
                    .unwrap()
                    .assembled()
                    .floppy_name(assembly::DeviceId::Fdc, 0),
                "first.img"
            );
            assert_eq!(
                shell
                    .presenter
                    .as_ref()
                    .unwrap()
                    .assembled_ref()
                    .machine
                    .virtual_time(),
                0
            );
            if failure != "changed pin" {
                shell.insert_dialog.as_mut().unwrap().journal = false;
                shell.confirm_insert();
                assert!(shell.insert_dialog.is_none());
                assert_eq!(
                    shell.presenter.as_ref().unwrap().media_events,
                    [(
                        unemu_core_api::SlotId(0),
                        unemu_core_api::MediaAction::Insert(2)
                    )]
                );
            }
            assert!(shell.stop());
        }
    }

    #[test]
    fn create_blank_disk_prefills_the_selected_failed_drives_path_and_geometry() {
        let temp = machine_file::test_support::TempDir::new("shell-blank-prefill");
        let root = temp.path().canonicalize().unwrap();
        let machine = root.join("example.toml");
        std::fs::write(&machine, "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='360K'\nimage='transfer.img'\nwrite='readonly'\n").unwrap();
        machine_file::resolve(&machine).unwrap();
        let mut shell = shell();
        shell.presenter = None;
        let mut entry = LibraryEntry::pending(machine);
        entry.checked = true;
        entry.warning = Some("cannot read image transfer.img".into());
        shell.library = vec![entry];
        shell.selected = Some(0);
        shell.command(Command::CreateBlank, &egui::Context::default());
        let form = shell.blank_disk.as_ref().unwrap();
        assert_eq!(form.directory, root);
        assert_eq!(form.command().unwrap().output, root.join("transfer.img"));
        assert_eq!(form.format, 4);
    }

    #[test]
    fn skipped_firmware_entries_are_collected_once_outside_machine_rows() {
        let mut shell = shell();
        shell.presenter = None;
        for path in ["first.toml", "second.toml"] {
            let mut entry = LibraryEntry::pending(path.into());
            entry.checked = true;
            entry.warning = None;
            entry.discovery_warnings = vec!["skipped unreadable entry".into()];
            shell.library.push(LibraryEntry::pending(path.into()));
            let index = shell.library.len() - 1;
            shell.apply_readiness(index, shell.library_revision, (entry, None));
        }
        assert_eq!(shell.discovery_warnings.len(), 1);
        assert!(shell.library.iter().all(|entry| !entry.has_problem()));
    }

    #[test]
    fn failed_journal_mount_stops_execution_and_shows_a_persistence_dialog() {
        let temp = machine_file::test_support::TempDir::new("shell-journal-error");
        let path = temp.path().join("extra.img");
        let journal = temp.path().join("invalid.upfj");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        std::fs::write(&journal, b"invalid journal").unwrap();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        draft
            .slot
            .entries
            .push(crate::media_dialog::browse(&path, draft.slot.geometry).unwrap());
        draft.slot.journal_retention = 2;
        draft.journal = true;
        draft.journal_path = journal.to_string_lossy().into_owned();
        draft.selected = Some(2);
        shell.confirm_insert();
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        assert!(!shell.presenter.as_ref().unwrap().running);
        assert!(shell.dialog.as_ref().unwrap().1.contains("journal mount"));
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled_ref()
                .machine
                .virtual_time(),
            0
        );
        assert_eq!(std::fs::read(&path).unwrap(), vec![0; 1_474_560]);
        assert_eq!(std::fs::read(&journal).unwrap(), b"invalid journal");
    }

    #[test]
    fn media_dialog_cancel_preserves_the_medium_and_insert_and_eject_wait_for_the_boundary() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().selected = Some(1);
        shell.insert_dialog = None;
        assert!(shell.presenter.as_ref().unwrap().media_events.is_empty());
        assert_eq!(
            shell
                .presenter
                .as_mut()
                .unwrap()
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "first.img"
        );
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().selected = Some(1);
        shell.confirm_insert();
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "first.img"
        );
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(1)
            )]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        shell.command(Command::Eject(0), &ctx);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Eject
            )]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "empty"
        );
    }

    #[test]
    fn browse_insertion_runs_while_paused_and_a_changed_pin_stays_in_the_dialog() {
        let ctx = egui::Context::default();
        for changed in [false, true] {
            let temp = machine_file::test_support::TempDir::new("shell-browse");
            let path = temp.path().join("extra.img");
            std::fs::write(&path, vec![0; 1_474_560]).unwrap();
            let mut shell = media_shell();
            shell.paused = true;
            shell.command(Command::InsertFile(0), &ctx);
            let draft = shell.insert_dialog.as_mut().unwrap();
            draft
                .slot
                .entries
                .push(crate::media_dialog::browse(&path, draft.slot.geometry).unwrap());
            draft.selected = Some(2);
            if changed {
                std::fs::write(&path, vec![1; 1_474_560]).unwrap();
            }
            shell.confirm_insert();
            let time = shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled_ref()
                .machine
                .virtual_time();
            shell.tick(&ctx, false, std::time::Duration::ZERO);
            assert_eq!(
                shell
                    .presenter
                    .as_ref()
                    .unwrap()
                    .assembled_ref()
                    .machine
                    .virtual_time(),
                time
            );
            if changed {
                assert!(shell
                    .insert_dialog
                    .as_ref()
                    .unwrap()
                    .error
                    .as_ref()
                    .unwrap()
                    .contains("SHA-256"));
                assert!(shell.dialog.is_none());
                assert!(shell.presenter.as_ref().unwrap().running);
            } else {
                assert_eq!(
                    shell
                        .presenter
                        .as_mut()
                        .unwrap()
                        .assembled()
                        .floppy_name(assembly::DeviceId::Fdc, 0),
                    "extra.img"
                );
                assert!(shell.paused);
            }
        }
    }

    #[test]
    fn starting_while_ctrl_or_enter_is_held_never_injects_that_key() {
        let ctx = egui::Context::default();
        for key in [KeyCode::ControlLeft, KeyCode::Enter] {
            let mut shell = keyboard_shell();
            let presenter = shell.presenter.take().unwrap();
            shell.release();
            let mut entry = LibraryEntry::pending("synthetic.toml".into());
            entry.warning = None;
            entry.checked = true;
            shell.library.push(entry);
            shell.selected = Some(0);
            shell.key(key, Pressed, false, &ctx);
            if key == KeyCode::ControlLeft {
                shell.start();
            }
            assert_eq!(shell.start_request, Some(0));
            shell
                .events
                .borrow_mut()
                .keyboard(key, Pressed, true, false, Some(key));
            shell.started(presenter);
            assert_eq!(shell.modifiers, [false; 8]);
            assert!(shell.held.is_empty());
            assert!(shell.consumed.is_empty());
            shell
                .events
                .borrow_mut()
                .keyboard(key, Pressed, true, false, Some(key));
            shell.events.borrow_mut().key(key, Released);
            shell.input(&ctx, false);
            assert!(take_guest_scancodes(&mut shell).is_empty(), "{key:?}");
            assert!(shell.guest_held.is_empty());
            shell.stop();
            assert_eq!(shell.modifiers, [false; 8]);
            assert!(shell.held.is_empty());
        }
    }

    #[test]
    fn a_modifier_pressed_before_focus_gain_has_no_guest_make_or_break() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.release();
        shell.key(KeyCode::ControlLeft, Pressed, false, &ctx);
        shell.events.borrow_mut().key(KeyCode::ControlLeft, Pressed);
        shell.focus_guest();
        assert!(shell.held.is_empty());
        assert_eq!(shell.modifiers, [false; 8]);
        shell.events.borrow_mut().keyboard(
            KeyCode::ControlLeft,
            Pressed,
            false,
            true,
            Some(KeyCode::ControlLeft),
        );
        shell.events.borrow_mut().keyboard(
            KeyCode::ControlLeft,
            Pressed,
            true,
            false,
            Some(KeyCode::ControlLeft),
        );
        shell
            .events
            .borrow_mut()
            .key(KeyCode::ControlLeft, Released);
        shell.input(&ctx, false);
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell.key(KeyCode::KeyC, Pressed, false, &ctx);
        shell.key(KeyCode::KeyC, Released, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0x2E, 0xAE]);
    }

    #[test]
    fn ctrl_c_inside_guest_delivers_balanced_physical_scan_codes() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        for (key, state) in [
            (KeyCode::ControlLeft, Pressed),
            (KeyCode::KeyC, Pressed),
            (KeyCode::KeyC, Released),
            (KeyCode::ControlLeft, Released),
        ] {
            shell.events.borrow_mut().key(key, state);
        }
        shell.input(&ctx, false);
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x2E, 0xAE, 0x9D]);
        assert!(shell.guest_held.is_empty());
        assert!(shell.held.is_empty());
    }

    /// While the machine has the keyboard, Ctrl+C is the guest's in both
    /// shortcut styles: the window keeps the keys from egui, the frame's
    /// clipboard step leaves the platform's clipboard chords alone while no
    /// text field is focused, and no clipboard is read or written.
    #[test]
    fn ctrl_c_stays_the_guests_and_no_clipboard_is_touched_without_a_text_field() {
        use ui_kit::text_clipboard::fake::{press, release, Host, CTRL_COMMAND, MAC_COMMAND};
        for (macos, modifiers) in [(false, CTRL_COMMAND), (true, MAC_COMMAND)] {
            let mut shell = keyboard_shell();
            shell.macos_shortcuts = macos;
            let mut frame = eframe::Frame::_new_kittest();
            let mut host = Host::new("never read");
            shell.clipboard = Box::new(host.clipboard.clone());
            let mut forwarded = false;
            for (key, state) in [
                (KeyCode::ControlLeft, Pressed),
                (KeyCode::KeyC, Pressed),
                (KeyCode::KeyC, Released),
                (KeyCode::ControlLeft, Released),
            ] {
                forwarded |= shell.events.borrow_mut().key(key, state);
            }
            assert!(!forwarded, "the guest's keys never reach egui");
            // The platform's clipboard chords as egui receives them while the
            // shell, and no text field, has the keyboard.
            let chords = [egui::Key::C, egui::Key::V, egui::Key::X]
                .into_iter()
                .flat_map(|key| [press(key, modifiers), release(key, modifiers)])
                .collect::<Vec<_>>();
            let mut seen = Vec::new();
            host.pass(chords.clone(), |ui| {
                eframe::App::logic(&mut shell, ui.ctx(), &mut frame);
                seen = ui.input(|input| input.events.clone());
            });
            assert_eq!(seen, chords);
            assert_eq!((host.reads(), host.writes()), (0, 0));
            shell.input(&host.ctx, false);
            assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x2E, 0xAE, 0x9D]);
        }
    }

    #[test]
    fn queued_keys_from_a_previous_session_never_enter_the_next_machine() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.events.borrow_mut().key(KeyCode::ControlLeft, Pressed);
        shell.events.borrow_mut().key(KeyCode::KeyC, Pressed);
        let presenter = shell.presenter.take().unwrap();
        shell.stop();
        shell.started(presenter);
        shell
            .events
            .borrow_mut()
            .key(KeyCode::ControlLeft, Released);
        shell.events.borrow_mut().key(KeyCode::KeyC, Released);
        shell.input(&ctx, false);
        assert!(take_guest_scancodes(&mut shell).is_empty());
    }

    fn logical_key(
        shell: &mut Shell,
        physical: KeyCode,
        logical: &str,
        state: ElementState,
        ctx: &egui::Context,
    ) {
        shell.keyboard(
            HostKey {
                key: physical,
                shortcut_key: shell_model::logical_shortcut_key(&winit::keyboard::Key::Character(
                    logical.into(),
                )),
                state,
                repeat: shell.held.contains(&physical) && state == Pressed,
                synthetic: false,
                guest_focus: shell.guest_focus,
                session: shell.session,
            },
            false,
            ctx,
        );
    }

    #[test]
    fn macos_command_state_survives_guest_focus_resets() {
        use KeyCode::*;
        for reset in ["click", "start", "focus", "menu"] {
            let mut shell = keyboard_shell();
            shell.macos_shortcuts = true;
            let ctx = egui::Context::default();
            shell.release();
            shell.key(SuperLeft, Pressed, false, &ctx);
            match reset {
                "start" => {
                    let presenter = shell.presenter.take().unwrap();
                    shell.started(presenter);
                }
                "focus" => {
                    shell
                        .events
                        .borrow_mut()
                        .forward(&WindowEvent::Focused(true));
                    shell.input(&ctx, false);
                    shell.focus_guest();
                }
                "menu" => {
                    for _ in 0..3 {
                        shell.release();
                    }
                    shell.focus_guest();
                }
                _ => shell.focus_guest(),
            }
            assert!(shell.host_modifiers.super_key(), "{reset}");
            shell.key(KeyA, Pressed, false, &ctx);
            shell.key(KeyA, Released, false, &ctx);
            assert!(take_guest_scancodes(&mut shell).is_empty(), "{reset}");
            shell.key(KeyK, Pressed, false, &ctx);
            assert_eq!(shell.state(), State::Stopped, "{reset}");
        }
    }

    #[test]
    fn macos_modifiers_changed_alone_routes_shift_chords_and_ends_fast_forward() {
        use KeyCode::*;
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        shell.focus_lost();
        for event in [
            WindowEvent::Focused(true),
            WindowEvent::ModifiersChanged(winit::keyboard::ModifiersState::SUPER.into()),
        ] {
            shell.events.borrow_mut().forward(&event);
        }
        shell.input(&ctx, false);
        shell.focus_guest();
        logical_key(&mut shell, KeyY, "t", Pressed, &ctx);
        assert_eq!(
            shell.presenter.as_ref().unwrap().pacing.effective(),
            crate::pacing::Speed::Unlimited
        );
        logical_key(&mut shell, KeyY, "t", Released, &ctx);
        assert_eq!(
            shell.presenter.as_ref().unwrap().pacing.effective(),
            crate::pacing::Speed::Normal
        );
        logical_key(&mut shell, KeyY, "t", Pressed, &ctx);
        shell
            .events
            .borrow_mut()
            .forward(&WindowEvent::ModifiersChanged(
                winit::keyboard::ModifiersState::empty().into(),
            ));
        shell.input(&ctx, false);
        assert_eq!(
            shell.presenter.as_ref().unwrap().pacing.effective(),
            crate::pacing::Speed::Normal
        );
        logical_key(&mut shell, KeyY, "t", Released, &ctx);
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell.focus_lost();
        shell
            .events
            .borrow_mut()
            .forward(&WindowEvent::Focused(true));
        shell
            .events
            .borrow_mut()
            .forward(&WindowEvent::ModifiersChanged(
                (ModifiersState::SUPER | ModifiersState::SHIFT).into(),
            ));
        shell.input(&ctx, false);
        shell.focus_guest();
        shell.presenter.as_mut().unwrap().audio.activate_for_test();
        logical_key(&mut shell, Semicolon, "M", Pressed, &ctx);
        logical_key(&mut shell, Semicolon, "M", Released, &ctx);
        assert!(shell.presenter.as_ref().unwrap().audio.muted());
        assert!(shell.guest_focus);
        assert!(take_guest_scancodes(&mut shell).is_empty());
    }

    #[test]
    fn macos_shortcuts_follow_logical_azerty_keys_and_guest_input_stays_physical() {
        use KeyCode::*;
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        ctx.begin_pass(Default::default());
        shell.key(SuperLeft, Pressed, false, &ctx);
        for (physical, logical) in [(KeyQ, "a"), (KeyW, "z")] {
            logical_key(&mut shell, physical, logical, Pressed, &ctx);
            logical_key(&mut shell, physical, logical, Released, &ctx);
            assert_eq!(shell.state(), State::Running);
            assert_eq!(shell.stop_prompt, None);
        }
        // AZERTY's physical A types "q": Command+Q asks before quitting.
        logical_key(&mut shell, KeyA, "q", Pressed, &ctx);
        logical_key(&mut shell, KeyA, "q", Released, &ctx);
        assert_eq!(shell.state(), State::Running);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        shell.answer_stop_prompt(false, &ctx);
        logical_key(&mut shell, KeyM, ",", Pressed, &ctx);
        logical_key(&mut shell, KeyM, ",", Released, &ctx);
        assert!(!shell.guest_focus);
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell.focus_guest();
        shell.presenter.as_mut().unwrap().audio.activate_for_test();
        shell.key(ShiftLeft, Pressed, false, &ctx);
        logical_key(&mut shell, Semicolon, "M", Pressed, &ctx);
        logical_key(&mut shell, Semicolon, "M", Released, &ctx);
        shell.key(ShiftLeft, Released, false, &ctx);
        assert!(shell.presenter.as_ref().unwrap().audio.muted());
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell.key(SuperLeft, Released, false, &ctx);
        logical_key(&mut shell, KeyQ, "a", Pressed, &ctx);
        logical_key(&mut shell, KeyQ, "a", Released, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0x10, 0x90]);
        shell.key(SuperLeft, Pressed, false, &ctx);
        logical_key(&mut shell, KeyZ, "w", Pressed, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        shell.answer_stop_prompt(true, &ctx);
        assert_eq!(shell.state(), State::Stopped);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(commands.contains(&egui::ViewportCommand::Close));
        assert!(!commands.contains(&egui::ViewportCommand::Minimized(true)));
    }

    #[test]
    fn macos_command_first_suppresses_shift_but_shift_first_remains_guest_input() {
        use KeyCode::*;
        for shift in [ShiftLeft, ShiftRight] {
            for command_first in [false, true] {
                let mut shell = keyboard_shell();
                shell.macos_shortcuts = true;
                shell.presenter.as_mut().unwrap().audio.activate_for_test();
                let ctx = egui::Context::default();
                for key in if command_first {
                    [SuperLeft, shift]
                } else {
                    [shift, SuperLeft]
                } {
                    shell.key(key, Pressed, false, &ctx);
                }
                shell.key(KeyM, Pressed, false, &ctx);
                shell.key(KeyM, Released, false, &ctx);
                shell.key(shift, Released, false, &ctx);
                shell.key(SuperLeft, Released, false, &ctx);
                assert!(shell.presenter.as_ref().unwrap().audio.muted());
                let expected = if command_first {
                    Vec::new()
                } else {
                    [Pressed, Released]
                        .into_iter()
                        .flat_map(|state| crate::set1_scancodes(shift, state).unwrap())
                        .collect()
                };
                assert_eq!(take_guest_scancodes(&mut shell), expected);
            }
        }
        assert!(shortcuts_help(true).contains("Press ⌘ first"));
    }

    #[test]
    fn macos_command_shortcuts_route_actions_without_guest_key_transitions() {
        use KeyCode::*;
        for (key, shift, expected) in [
            (KeyF, false, Command::Next(0)),
            (KeyF, true, Command::Next(1)),
            (KeyE, false, Command::Eject(0)),
            (KeyE, true, Command::Eject(1)),
            (KeyI, false, Command::Insert(0)),
            (KeyI, true, Command::Insert(1)),
            (KeyM, true, Command::Mute),
            (KeyT, false, Command::FastForward),
            (KeyG, false, Command::Release),
            (KeyP, false, Command::Pause),
            (KeyK, false, Command::Stop),
            (End, false, Command::Reset),
            (Enter, false, Command::Fullscreen),
            (KeyN, false, Command::Placeholder("New machine")),
            (KeyO, false, Command::Open),
            (Comma, false, Command::Settings),
            (Slash, true, Command::Shortcuts),
            (KeyQ, false, Command::Exit),
            (KeyW, false, Command::Exit),
            (KeyM, false, Command::Minimize),
        ] {
            for command_key in [SuperLeft, SuperRight] {
                let mut shell = media_shell();
                *shell.presenter.as_mut().unwrap().assembled() =
                    machine_file::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
                shell.macos_shortcuts = true;
                let presenter = shell.presenter.as_mut().unwrap();
                presenter.media_slots[1] = presenter.media_slots[0].clone();
                presenter.audio.activate_for_test();
                let ctx = egui::Context::default();
                ctx.begin_pass(egui::RawInput::default());
                shell.key(command_key, Pressed, false, &ctx);
                assert!(take_guest_scancodes(&mut shell).is_empty());
                if shift {
                    shell.key(ShiftLeft, Pressed, false, &ctx);
                    assert!(take_guest_scancodes(&mut shell).is_empty());
                }
                assert_eq!(
                    shell_model::hotkey(key, shell.modifiers, shell.state(), true, true),
                    Some(expected)
                );
                shell.key(key, Pressed, false, &ctx);
                match expected {
                    Command::Next(unit) | Command::Eject(unit) => {
                        assert_eq!(
                            shell.presenter.as_ref().unwrap().media_events,
                            [(
                                SlotId(unit),
                                if matches!(expected, Command::Next(_)) {
                                    MediaAction::Next
                                } else {
                                    MediaAction::Eject
                                }
                            )]
                        );
                    }
                    Command::Insert(unit) => {
                        assert_eq!(shell.picker.as_ref().unwrap().unit, unit)
                    }
                    Command::Mute => assert!(shell.presenter.as_ref().unwrap().audio.muted()),
                    Command::FastForward => assert_eq!(
                        shell.presenter.as_ref().unwrap().pacing.effective(),
                        crate::pacing::Speed::Unlimited
                    ),
                    Command::Pause => assert!(shell.paused),
                    Command::Stop => assert!(shell.presenter.is_none()),
                    Command::Exit => {
                        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
                        assert!(shell.presenter.is_some());
                        shell.answer_stop_prompt(true, &ctx);
                        assert!(shell.presenter.is_none());
                    }
                    Command::Fullscreen => assert!(shell.fullscreen),
                    Command::Placeholder("New machine") => {
                        assert!(shell.image_library.new_machine.is_some());
                        shell.image_library.close_dialogs();
                    }
                    Command::Placeholder(_) | Command::Shortcuts => assert!(shell.dialog.is_some()),
                    // The release's NES core opens files: the host's dialog
                    // asks for one.
                    Command::Open => assert!(shell.file_dialogs.is_open()),
                    Command::Minimize => assert!(!shell.guest_focus),
                    Command::Settings => assert!(!shell.guest_focus),
                    Command::Release => {
                        assert!(!shell.presenter.as_ref().unwrap().mouse.captured())
                    }
                    Command::Reset => assert_eq!(
                        take_guest_scancodes(&mut shell),
                        [0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D]
                    ),
                    _ => unreachable!(),
                }
                if shell.presenter.is_some() {
                    assert!(take_guest_scancodes(&mut shell).is_empty(), "{expected:?}");
                }
                shell.key(key, Pressed, false, &ctx);
                for released in [command_key, ShiftLeft, key] {
                    shell.key(released, Released, false, &ctx);
                }
                if shell.presenter.is_some() {
                    assert!(take_guest_scancodes(&mut shell).is_empty(), "{expected:?}");
                }
                assert!(shell.guest_held.is_empty());
                let mut output = ctx.end_pass();
                output.textures_delta.clear();
                let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
                match expected {
                    Command::Exit => assert!(commands.contains(&egui::ViewportCommand::Close)),
                    Command::Minimize => {
                        assert!(commands.contains(&egui::ViewportCommand::Minimized(true)))
                    }
                    Command::Fullscreen => {
                        assert!(commands.contains(&egui::ViewportCommand::Fullscreen(true)))
                    }
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn macos_ctrl_alt_and_the_old_host_chords_remain_immediate_guest_input() {
        use KeyCode::*;
        for (ctrl, alt, prefix, suffix) in [
            (ControlLeft, AltLeft, vec![0x1D, 0x38], vec![0xB8, 0x9D]),
            (
                ControlRight,
                AltRight,
                vec![0xE0, 0x1D, 0xE0, 0x38],
                vec![0xE0, 0xB8, 0xE0, 0x9D],
            ),
        ] {
            for key in [
                KeyF, KeyE, KeyI, KeyM, KeyT, KeyG, KeyP, KeyQ, KeyN, KeyO, Comma, F1, Enter, End,
                Delete,
            ] {
                let mut shell = keyboard_shell();
                shell.macos_shortcuts = true;
                let ctx = egui::Context::default();
                shell.key(ctrl, Pressed, false, &ctx);
                assert_eq!(take_guest_scancodes(&mut shell), prefix[..prefix.len() / 2]);
                shell.key(alt, Pressed, false, &ctx);
                assert_eq!(take_guest_scancodes(&mut shell), prefix[prefix.len() / 2..]);
                shell.key(key, Pressed, false, &ctx);
                assert_eq!(
                    take_guest_scancodes(&mut shell),
                    crate::set1_scancodes(key, Pressed).unwrap()
                );
                shell.key(key, Released, false, &ctx);
                assert_eq!(
                    take_guest_scancodes(&mut shell),
                    crate::set1_scancodes(key, Released).unwrap()
                );
                shell.key(alt, Released, false, &ctx);
                shell.key(ctrl, Released, false, &ctx);
                assert_eq!(take_guest_scancodes(&mut shell), suffix);
                assert_eq!(shell.state(), State::Running);
                assert!(shell.dialog.is_none());
            }
        }
    }

    /// A running keyboard shell on macOS whose system-shortcut tap is a
    /// recording fake.
    fn system_keys_shell() -> (Shell, Rc<RefCell<crate::system_keys::fake::Log>>) {
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let (keys, log) = crate::system_keys::fake::system_keys();
        shell.system_keys = keys;
        (shell, log)
    }

    /// One key event the tap took from macOS; false when the shell refused it.
    fn tapped(
        log: &Rc<RefCell<crate::system_keys::fake::Log>>,
        key: KeyCode,
        state: ElementState,
        repeat: bool,
    ) -> bool {
        let mut deliver = log.borrow_mut().deliver.take().expect("tap installed");
        let taken = deliver(crate::system_keys::GuestKey { key, state, repeat });
        log.borrow_mut().deliver = Some(deliver);
        taken
    }

    #[test]
    fn the_system_shortcut_tap_exists_only_while_a_running_guest_has_the_keyboard() {
        let ctx = egui::Context::default();
        let (mut shell, log) = system_keys_shell();
        shell.input(&ctx, false);
        assert_eq!(log.borrow().installs, 0, "off by default");
        assert!(!shell.events.borrow().system_keys);
        for give_back in [
            "pause",
            "dialog",
            "focus",
            "hidden",
            "minimized",
            "stop",
            "option",
            "host",
        ] {
            let (mut shell, log) = system_keys_shell();
            shell.preferences.send_system_shortcuts = true;
            shell.input(&ctx, false);
            assert!(shell.system_keys.installed(), "{give_back}");
            assert!(shell.events.borrow().system_keys);
            match give_back {
                "pause" => shell.paused = true,
                "dialog" => shell.dialog = Some(("Synthetic", String::new())),
                "focus" => {
                    shell
                        .events
                        .borrow_mut()
                        .forward(&WindowEvent::Focused(false));
                }
                "hidden" => shell.hidden = true,
                "minimized" => shell.minimized = true,
                "stop" => {
                    shell.stop();
                }
                "option" => shell.preferences.send_system_shortcuts = false,
                // Other hosts have no system shortcuts to take.
                _ => shell.macos_shortcuts = false,
            }
            shell.input(&ctx, false);
            assert!(!shell.system_keys.installed(), "{give_back}");
            assert!(!shell.events.borrow().system_keys, "{give_back}");
            assert_eq!(log.borrow().removes, 1, "{give_back}");
            assert!(log.borrow().deliver.is_none(), "{give_back}");
        }
        // Quitting drops the shell, which removes an installed tap.
        let (mut shell, log) = system_keys_shell();
        shell.preferences.send_system_shortcuts = true;
        shell.input(&ctx, false);
        drop(shell);
        assert_eq!((log.borrow().installs, log.borrow().removes), (1, 1));
    }

    #[test]
    fn a_refused_tap_explains_the_permission_once_in_the_status_bar() {
        let ctx = egui::Context::default();
        let (mut shell, log) = system_keys_shell();
        log.borrow_mut().refuse = true;
        shell.preferences.send_system_shortcuts = true;
        shell.input(&ctx, false);
        assert_eq!(shell.message, crate::system_keys::REFUSED_HINT);
        assert!(!shell.system_keys.installed());
        shell.message.clear();
        shell.release();
        shell.focus_guest();
        shell.input(&ctx, false);
        assert_eq!(log.borrow().installs, 2, "the next focus tries again");
        assert!(shell.message.is_empty(), "once per session");
    }

    #[test]
    fn ctrl_arrows_taken_from_macos_reach_the_guest_and_focus_loss_releases_them() {
        use KeyCode::*;
        let ctx = egui::Context::default();
        let (mut shell, log) = system_keys_shell();
        shell.preferences.send_system_shortcuts = true;
        shell.input(&ctx, false);
        // The window still receives Ctrl itself; the tap takes the arrow.
        shell.events.borrow_mut().key(ControlLeft, Pressed);
        assert!(tapped(&log, ArrowLeft, Pressed, false));
        assert!(tapped(&log, ArrowLeft, Pressed, true));
        shell.input(&ctx, false);
        let mut first = vec![0x1D];
        first.extend(crate::set1_scancodes(ArrowLeft, Pressed).unwrap());
        let made = take_guest_scancodes(&mut shell);
        assert!(
            made.starts_with(&first) && made.len() > first.len(),
            "{made:x?}"
        );
        assert!(shell.guest_held.contains(&ArrowLeft));
        // Focus moves to another application while both keys are down.
        shell
            .events
            .borrow_mut()
            .forward(&WindowEvent::Focused(false));
        shell.input(&ctx, false);
        let released = take_guest_scancodes(&mut shell);
        let arrow_break = crate::set1_scancodes(ArrowLeft, Released).unwrap();
        assert!(
            released
                .windows(arrow_break.len())
                .any(|w| w == arrow_break),
            "{released:x?}"
        );
        assert!(released.contains(&0x9D), "{released:x?}");
        assert!(shell.guest_held.is_empty() && shell.held.is_empty());
        assert_eq!(log.borrow().removes, 1);
        // The late releases macOS delivers to the window change nothing.
        shell.events.borrow_mut().key(ArrowLeft, Released);
        shell.events.borrow_mut().key(ControlLeft, Released);
        shell.input(&ctx, false);
        assert!(take_guest_scancodes(&mut shell).is_empty());
    }

    #[test]
    fn the_tap_hands_keys_back_to_macos_when_the_shell_cannot_take_them() {
        let ctx = egui::Context::default();
        let (mut shell, log) = system_keys_shell();
        shell.preferences.send_system_shortcuts = true;
        shell.input(&ctx, false);
        let busy = shell.events.borrow_mut();
        assert!(!tapped(&log, KeyCode::ArrowUp, Pressed, false));
        drop(busy);
        shell.events.borrow_mut().guest_focus = false;
        assert!(!tapped(&log, KeyCode::ArrowUp, Pressed, false));
        assert!(shell.events.borrow().queue.is_empty());
    }

    #[test]
    fn a_save_without_a_preferences_file_gives_the_startup_reason() {
        let mut shell = shell();
        shell.save_preferences();
        assert_eq!(
            shell.dialog.as_ref().map(|(_, text)| text.as_str()),
            Some("Cannot save preferences: platform config directory is unavailable")
        );
        let reason = "copying preferences from /a/preferences.toml to /b/preferences.toml: \
                      permission denied";
        shell.preferences_path_error = Some(reason.into());
        shell.save_preferences();
        assert_eq!(
            shell.dialog.as_ref().map(|(_, text)| text.clone()),
            Some(format!("Cannot save preferences: {reason}"))
        );
    }

    #[test]
    fn a_failed_save_still_asks_for_access_and_reports_both() {
        let (mut shell, log) = system_keys_shell();
        shell.preferences_path = None;
        shell.preferences_path_error = Some("copying preferences: permission denied".into());
        shell.open_preferences();
        shell.set_system_shortcuts(true);
        assert!(
            shell.preferences.send_system_shortcuts,
            "on for this session"
        );
        assert_eq!(log.borrow().access_requests, 1);
        let dialog = shell.preferences_dialog.as_ref().unwrap();
        let note = dialog
            .system_shortcuts
            .as_ref()
            .unwrap()
            .note
            .clone()
            .unwrap();
        assert!(
            note.starts_with("Cannot save preferences: copying preferences: permission denied"),
            "{note}"
        );
        assert!(note.ends_with(ACCESS_NOTE), "{note}");
    }

    #[test]
    fn turning_the_option_on_asks_for_accessibility_once_and_saves_it() {
        let temp = machine_file::test_support::TempDir::new("shell-system-shortcuts");
        let path = temp.path().join("preferences.toml");
        let (mut shell, log) = system_keys_shell();
        shell.preferences_path = Some(path.clone());
        shell.open_preferences();
        let note = |shell: &Shell| {
            let shortcuts = shell.preferences_dialog.as_ref().unwrap();
            let shortcuts = shortcuts.system_shortcuts.as_ref().unwrap();
            (shortcuts.on, shortcuts.note.clone())
        };
        assert_eq!(note(&shell), (false, None));
        assert_eq!(log.borrow().access_requests, 0);
        shell.set_system_shortcuts(true);
        assert_eq!(log.borrow().access_requests, 1);
        assert_eq!(note(&shell), (true, Some(ACCESS_NOTE.into())));
        assert!(
            crate::preferences::Preferences::load(&path)
                .unwrap()
                .send_system_shortcuts
        );
        assert_eq!(log.borrow().installs, 0, "not while Preferences is open");
        shell.set_system_shortcuts(false);
        assert_eq!(log.borrow().access_requests, 1, "asks only when turned on");
        assert_eq!(note(&shell), (false, None));
        assert!(
            !crate::preferences::Preferences::load(&path)
                .unwrap()
                .send_system_shortcuts
        );
        // Reopening Preferences still says what is missing.
        shell.set_system_shortcuts(true);
        assert_eq!(log.borrow().access_requests, 2);
        shell.close_preferences();
        shell.open_preferences();
        assert_eq!(note(&shell), (true, Some(ACCESS_NOTE.into())));
        log.borrow_mut().access_granted = true;
        shell.set_system_shortcuts(true);
        assert_eq!(note(&shell), (true, None));
        shell.close_preferences();
        shell.focus_guest();
        shell.input(&egui::Context::default(), false);
        assert!(shell.system_keys.installed());
        // Once a tap exists the privilege is known to be there.
        log.borrow_mut().access_granted = false;
        shell.open_preferences();
        assert_eq!(note(&shell), (true, None));
        shell.close_preferences();
        // Other hosts do not offer the option.
        shell.macos_shortcuts = false;
        shell.open_preferences();
        assert!(shell
            .preferences_dialog
            .as_ref()
            .unwrap()
            .system_shortcuts
            .is_none());
    }

    #[test]
    fn macos_command_hold_ends_on_either_key_release_or_focus_loss() {
        use KeyCode::*;
        for selected in crate::pacing::Speed::ALL {
            for end in [SuperLeft, KeyT, Escape] {
                let mut shell = keyboard_shell();
                shell.macos_shortcuts = true;
                let ctx = egui::Context::default();
                shell.command(Command::Speed(selected), &ctx);
                shell.key(SuperLeft, Pressed, false, &ctx);
                shell.key(KeyT, Pressed, false, &ctx);
                assert_eq!(
                    shell.presenter.as_ref().unwrap().pacing.effective(),
                    crate::pacing::Speed::Unlimited
                );
                if end == Escape {
                    shell.focus_lost();
                    shell.focus_guest();
                    shell.key(SuperLeft, Released, false, &ctx);
                } else {
                    shell.key(end, Released, true, &ctx);
                }
                assert_eq!(
                    shell.presenter.as_ref().unwrap().pacing.effective(),
                    selected
                );
                if end != KeyT {
                    shell.key(KeyT, Pressed, false, &ctx);
                }
                shell.key(KeyT, Released, false, &ctx);
                shell.key(SuperLeft, Released, false, &ctx);
                if end == Escape {
                    assert_eq!(take_guest_scancodes(&mut shell), [0x14, 0x94]);
                } else {
                    assert!(take_guest_scancodes(&mut shell).is_empty());
                }
                assert!(shell.modifiers.iter().all(|held| !held));
            }
        }
    }

    #[test]
    fn macos_modifiers_changed_on_refocus_suppresses_keys_without_restarting_a_hold() {
        use KeyCode::*;
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        shell.key(SuperLeft, Pressed, false, &ctx);
        shell.key(KeyT, Pressed, false, &ctx);
        shell.focus_lost();
        for event in [
            WindowEvent::ModifiersChanged(winit::keyboard::ModifiersState::empty().into()),
            WindowEvent::Focused(true),
            WindowEvent::ModifiersChanged(winit::keyboard::ModifiersState::SUPER.into()),
        ] {
            shell.events.borrow_mut().forward(&event);
        }
        shell.input(&ctx, false);
        shell.focus_guest();
        shell.keyboard(
            HostKey {
                key: KeyT,
                shortcut_key: Some(KeyT),
                state: Pressed,
                repeat: true,
                synthetic: false,
                guest_focus: true,
                session: shell.session,
            },
            false,
            &ctx,
        );
        assert_eq!(
            shell.presenter.as_ref().unwrap().pacing.effective(),
            crate::pacing::Speed::Normal
        );
        shell.key(KeyA, Pressed, false, &ctx);
        shell.key(KeyA, Released, false, &ctx);
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell
            .events
            .borrow_mut()
            .forward(&WindowEvent::ModifiersChanged(
                winit::keyboard::ModifiersState::empty().into(),
            ));
        shell.input(&ctx, false);
        shell.key(KeyA, Pressed, false, &ctx);
        shell.key(KeyA, Released, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0x1E, 0x9E]);
    }

    #[test]
    fn macos_ctrl_alt_g_keeps_capture_and_command_g_releases_it() {
        use KeyCode::*;
        let mut shell = captured_shell();
        *shell.presenter.as_mut().unwrap().assembled() =
            machine_file::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        for key in [ControlLeft, AltLeft, KeyG] {
            shell.key(key, Pressed, false, &ctx);
        }
        assert!(shell.presenter.as_ref().unwrap().mouse.captured());
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x38, 0x22]);
        shell.key(KeyG, Released, false, &ctx);
        shell.key(SuperLeft, Pressed, false, &ctx);
        // Ctrl and Alt remain guest-held until their physical releases.
        shell.key(ControlLeft, Released, false, &ctx);
        shell.key(AltLeft, Released, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0xA2, 0x9D, 0xB8]);
        shell.key(KeyG, Pressed, false, &ctx);
        assert!(!shell.presenter.as_ref().unwrap().mouse.captured());
        shell.key(SuperLeft, Released, false, &ctx);
        shell.key(KeyG, Released, false, &ctx);
        assert!(take_guest_scancodes(&mut shell).is_empty());
    }

    #[test]
    fn macos_unassigned_command_chords_and_focus_loss_never_type_guest_keys() {
        use KeyCode::*;
        for key in [KeyH, Tab, KeyA, ShiftLeft, ShiftRight, SuperRight] {
            let mut shell = keyboard_shell();
            shell.macos_shortcuts = true;
            let ctx = egui::Context::default();
            shell.key(SuperLeft, Pressed, false, &ctx);
            shell.key(key, Pressed, false, &ctx);
            shell.focus_lost();
            shell.focus_guest();
            shell.key(SuperLeft, Released, false, &ctx);
            shell.key(key, Released, false, &ctx);
            assert!(take_guest_scancodes(&mut shell).is_empty());
        }
        let help = shortcuts_help(true);
        assert!(help.contains("⌘T: hold to fast-forward"));
        assert!(help.contains("⌘⇧M: toggle user mute"));
        assert!(!help.contains("Ctrl+Alt+T"));
    }

    #[test]
    fn speed_cell_drops_measurement_after_halt_and_stop() {
        for selected in crate::pacing::Speed::ALL {
            let mut shell = shell();
            let presenter = shell.presenter.as_mut().unwrap();
            presenter.select_speed(selected, std::time::Duration::ZERO);
            presenter.pacing.observe(std::time::Duration::ZERO, 0);
            presenter.pacing.observe(
                std::time::Duration::from_secs(1),
                crate::pacing::virtual_ticks(std::time::Duration::from_secs(2)) as u64,
            );
            assert!(shell.speed_status().contains("2.0x"));
            shell.presenter.as_mut().unwrap().running = false;
            assert_eq!(shell.speed_status(), format!("Speed {}", selected.label()));
            shell.stop();
            assert_eq!(shell.speed_status(), "Speed 100%");
        }
    }

    #[test]
    fn speed_fast_forward_release_and_focus_loss_restore_the_selected_mode() {
        for selected in crate::pacing::Speed::ALL {
            for lose_focus in [false, true] {
                let mut shell = keyboard_shell();
                let ctx = egui::Context::default();
                shell.command(Command::Speed(selected), &ctx);
                for key in [KeyCode::ControlRight, KeyCode::AltRight, KeyCode::KeyT] {
                    shell.key(key, Pressed, false, &ctx);
                }
                shell.key(KeyCode::KeyT, Pressed, false, &ctx);
                let presenter = shell.presenter.as_ref().unwrap();
                assert_eq!(
                    presenter.pacing.effective(),
                    crate::pacing::Speed::Unlimited
                );
                assert!(presenter.audio.muted());
                assert!(presenter.pacing.status().starts_with("Fast-forward"));
                if lose_focus {
                    shell.focus_lost();
                } else {
                    shell.key(KeyCode::AltRight, Released, false, &ctx);
                    shell.key(KeyCode::ControlRight, Released, false, &ctx);
                    shell.key(KeyCode::KeyT, Released, true, &ctx);
                }
                let presenter = shell.presenter.as_ref().unwrap();
                assert_eq!(presenter.pacing.effective(), selected);
                assert_eq!(presenter.audio.muted(), !selected.audio_enabled());
                assert!(!presenter.pacing.status().starts_with("Fast-forward"));
            }
        }
        assert!(shortcuts_help(false).contains("Ctrl+Alt+T: hold to fast-forward"));
    }

    /// Ctrl+Alt+Shift+D (Command+Shift+D on macOS) saves the running PC's
    /// report as the next numbered file and the status line names it, with
    /// no environment switch, and as well with the pointer captured as
    /// without. Its D never reaches the guest: only the modifiers' make and
    /// break codes do. Without a machine the chord writes nothing.
    #[test]
    fn the_diagnostic_chord_saves_a_report_and_never_types_d() {
        let temp = machine_file::test_support::TempDir::new("shell-diagnostics");
        let folder = temp.path().join("unemu").join("diagnostics");
        let chord = |shell: &mut Shell| {
            let ctx = egui::Context::default();
            let keys: &[KeyCode] = if shell.macos_shortcuts {
                &[KeyCode::SuperLeft, KeyCode::ShiftLeft, KeyCode::KeyD]
            } else {
                &[
                    KeyCode::ControlLeft,
                    KeyCode::AltLeft,
                    KeyCode::ShiftLeft,
                    KeyCode::KeyD,
                ]
            };
            for &key in keys {
                shell.key(key, Pressed, false, &ctx);
            }
            for &key in keys.iter().rev() {
                shell.key(key, Released, false, &ctx);
            }
            take_guest_scancodes(shell)
        };
        let mut none = keyboard_shell();
        none.presenter = None;
        none.args.diagnostics = Ok(folder.clone());
        none.message = "Ready".into();
        let ctx = egui::Context::default();
        for key in [
            KeyCode::ControlLeft,
            KeyCode::AltLeft,
            KeyCode::ShiftLeft,
            KeyCode::KeyD,
        ] {
            none.key(key, Pressed, false, &ctx);
        }
        assert_eq!(none.message, "Ready");
        assert!(!folder.exists(), "no machine runs");

        let mut on = keyboard_shell();
        on.args.diagnostics = Ok(folder.clone());
        let typed = chord(&mut on);
        assert!(
            !typed.contains(&0x20) && !typed.contains(&0xA0),
            "{typed:x?}"
        );
        assert!(on.guest_held.is_empty());
        let first = folder.join("diag-1.txt");
        assert_eq!(
            on.message,
            format!("Machine report saved to {}", first.display())
        );
        let report = std::fs::read_to_string(&first).unwrap();
        assert!(
            report.starts_with("machine diagnostic report\n"),
            "{report}"
        );
        assert!(report.contains("\nstatus   ready\n"), "{report}");
        // The first report starts the delivery log, so it lists none yet.
        assert!(
            report.ends_with(
                "\ndesktop  running\n\
                 desktop  the delivery log starts with this report: the next one lists the deliveries since\n\
                 desktop  exceptions not recorded (Tools › Diagnostics › Record exceptions is off)\n"
            ),
            "{report}"
        );
        assert!(
            report.contains("\ndelivery the last 0 of 0 exceptions and interrupts in 0 entries"),
            "{report}"
        );
        assert!(report.contains("\nkbc      status "), "{report}");
        on.paused = true;
        chord(&mut on);
        let second = std::fs::read_to_string(folder.join("diag-2.txt")).unwrap();
        assert!(
            second.contains("\ndesktop  paused\ndesktop  exceptions not recorded"),
            "{second}"
        );
        assert!(on.message.ends_with("diag-2.txt"), "{}", on.message);

        // On macOS the chord is Command+Shift+D, and nothing of it is typed.
        let mut mac = keyboard_shell();
        mac.macos_shortcuts = true;
        mac.args.diagnostics = Ok(folder.clone());
        assert!(chord(&mut mac).is_empty());
        assert!(mac.message.ends_with("diag-3.txt"), "{}", mac.message);

        // With the pointer captured, through the window's event queue as
        // the window delivers keys and modifier changes.
        for (macos, number) in [(false, 4), (true, 5)] {
            let mut shell = captured_shell();
            shell.macos_shortcuts = macos;
            shell.args.diagnostics = Ok(folder.clone());
            let ctx = egui::Context::default();
            let keys: &[(KeyCode, ModifiersState)] = if macos {
                &[
                    (KeyCode::SuperLeft, ModifiersState::SUPER),
                    (KeyCode::ShiftLeft, ModifiersState::SHIFT),
                    (KeyCode::KeyD, ModifiersState::empty()),
                ]
            } else {
                &[
                    (KeyCode::ControlLeft, ModifiersState::CONTROL),
                    (KeyCode::AltLeft, ModifiersState::ALT),
                    (KeyCode::ShiftLeft, ModifiersState::SHIFT),
                    (KeyCode::KeyD, ModifiersState::empty()),
                ]
            };
            let mut flags = ModifiersState::empty();
            for &(key, flag) in keys {
                flags |= flag;
                let mut events = shell.events.borrow_mut();
                events.forward(&WindowEvent::ModifiersChanged(flags.into()));
                events.keyboard(key, Pressed, false, false, Some(key));
                drop(events);
                shell.input(&ctx, false);
            }
            assert!(shell.presenter.as_ref().unwrap().mouse.captured());
            let path = folder.join(format!("diag-{number}.txt"));
            assert_eq!(
                shell.message,
                format!("Machine report saved to {}", path.display()),
                "macOS {macos}"
            );
        }
    }

    #[test]
    fn speed_fast_forward_chord_never_delivers_t_to_the_guest() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyT] {
            shell.key(key, Pressed, false, &ctx);
        }
        shell.key(KeyCode::KeyT, Pressed, false, &ctx);
        for key in [KeyCode::AltLeft, KeyCode::ControlLeft, KeyCode::KeyT] {
            shell.key(key, Released, false, &ctx);
        }
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x38, 0xB8, 0x9D]);
        assert!(shell.guest_held.is_empty());
    }

    #[test]
    fn host_shortcuts_balance_forwarded_modifiers_without_the_command_key() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyP] {
            shell.key(key, Pressed, false, &ctx);
        }
        for key in [KeyCode::KeyP, KeyCode::AltLeft, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        assert!(shell.paused);
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x38, 0xB8, 0x9D]);
        shell.command(Command::Pause, &ctx);
        shell.key(KeyCode::ControlLeft, Pressed, false, &ctx);
        shell.key(KeyCode::KeyC, Pressed, false, &ctx);
        shell.focus_lost();
        shell.focus_guest();
        for key in [KeyCode::KeyC, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x2E, 0xAE, 0x9D]);
    }

    #[test]
    fn modal_input_does_not_leak_and_release_balances_held_guest_keys() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.key(KeyCode::KeyA, Pressed, false, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            1
        );
        shell.release();
        let pending = shell
            .presenter
            .as_ref()
            .unwrap()
            .keyboard()
            .pending_events();
        assert_eq!(pending, 2);
        shell.key(KeyCode::KeyB, Pressed, true, &ctx);
        shell.key(KeyCode::KeyB, Released, true, &ctx);
        shell.key(KeyCode::KeyA, Released, false, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            pending
        );
        assert!(!shell.guest_focus);
        assert!(shell.presenter.as_ref().unwrap().window.is_none());
    }

    #[test]
    fn a_shell_focused_dialog_shortcut_blocks_later_keys_in_the_same_batch() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.release();
        shell.key(KeyCode::F1, Pressed, false, &ctx);
        shell.key(KeyCode::KeyA, Pressed, false, &ctx);
        assert!(shell.dialog.is_some());
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            0
        );
    }

    #[test]
    fn shell_pause_shortcut_suppresses_repeat_and_break_until_release() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyP] {
            shell.key(key, Pressed, false, &ctx);
        }
        assert!(shell.paused);
        shell.key(KeyCode::KeyP, Pressed, false, &ctx);
        assert!(shell.paused);
        shell.key(KeyCode::KeyP, Released, false, &ctx);
        shell.key(KeyCode::KeyP, Pressed, false, &ctx);
        assert!(!shell.paused);
        shell.key(KeyCode::ControlLeft, Released, false, &ctx);
        shell.key(KeyCode::AltLeft, Released, false, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            4
        );
    }

    #[test]
    fn held_ctrl_alt_end_releases_forwarded_modifiers_before_guest_reset() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.key(KeyCode::ControlLeft, Pressed, false, &ctx);
        shell.key(KeyCode::AltLeft, Pressed, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x38]);
        shell.key(KeyCode::End, Pressed, false, &ctx);
        for key in [KeyCode::End, KeyCode::AltLeft, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        assert_eq!(
            take_guest_scancodes(&mut shell),
            [0xB8, 0x9D, 0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D]
        );
        assert!(shell.guest_held.is_empty());
    }

    #[test]
    fn reset_queues_balanced_guest_keys_and_stop_returns_to_the_library() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.command(Command::Reset, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            1
        );
        assert_eq!(
            take_guest_scancodes(&mut shell),
            [0x1D, 0x38, 0x53, 0xD3, 0xB8, 0x9D]
        );
        assert_eq!(shell.state(), State::Running);
        shell.command(Command::Pause, &ctx);
        assert_eq!(shell.state(), State::Paused);
        shell.command(Command::Stop, &ctx);
        assert_eq!(shell.state(), State::Stopped);
        assert!(shell.presenter.is_none());
    }

    #[test]
    fn raw_motion_coalesces_only_between_button_transitions() {
        let mut events = Events::default();
        events.motion(2.0, 3.0);
        events.motion(4.0, 5.0);
        events.window(WindowEvent::MouseInput {
            device_id: winit::event::DeviceId::dummy(),
            state: Pressed,
            button: winit::event::MouseButton::Left,
        });
        events.motion(7.0, 8.0);
        assert_eq!(events.queue.len(), 3);
        assert!(matches!(events.queue[0], HostEvent::Motion(6.0, 8.0)));
        assert!(matches!(events.queue[1], HostEvent::Window(_)));
        assert!(matches!(events.queue[2], HostEvent::Motion(7.0, 8.0)));
    }
    #[test]
    fn guest_keyboard_filter_keeps_navigation_out_of_egui_input() {
        let shell = shell();
        shell.sync_routing();
        let mut input = egui::RawInput::default();
        for (physical, logical) in [
            (KeyCode::ArrowUp, egui::Key::ArrowUp),
            (KeyCode::Tab, egui::Key::Tab),
            (KeyCode::Space, egui::Key::Space),
            (KeyCode::Enter, egui::Key::Enter),
            (KeyCode::Equal, egui::Key::Plus),
            (KeyCode::Digit0, egui::Key::Num0),
        ] {
            for state in [Pressed, Released] {
                if shell.events.borrow_mut().key(physical, state) {
                    input.events.push(egui::Event::Key {
                        key: logical,
                        physical_key: Some(logical),
                        pressed: state == Pressed,
                        repeat: false,
                        modifiers: egui::Modifiers::CTRL,
                    });
                }
            }
        }
        assert!(input.events.is_empty());
        assert_eq!(shell.events.borrow().queue.len(), 12);
        let ctx = egui::Context::default();
        let output = ctx.run_pass(input, |ctx| {
            assert!(ctx.input(|input| input.events.is_empty()))
        });
        output.drop_without_applying_deltas();
        let mut events = shell.events.borrow_mut();
        assert!(!events.forward(&WindowEvent::Ime(winit::event::Ime::Commit("text".into()))));
        assert!(events.forward(&WindowEvent::ModifiersChanged(Default::default())));
        events.guest_focus = false;
        assert!(events.key(KeyCode::F1, Pressed));
        assert!(events.forward(&WindowEvent::Ime(winit::event::Ime::Enabled)));
    }

    #[test]
    fn screen_press_claims_keyboard_before_the_next_repaint() {
        let mut events = Events {
            screen: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::splat(100.0),
            )),
            pixels_per_point: 1.0,
            ..Default::default()
        };
        events.forward(&WindowEvent::CursorMoved {
            device_id: winit::event::DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(50.0, 50.0),
        });
        events.forward(&button(winit::event::MouseButton::Left, Pressed));
        assert!(!events.key(KeyCode::Enter, Pressed));
    }

    #[test]
    fn guest_f1_and_control_shortcuts_queue_balanced_scan_transitions() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        for state in [Pressed, Released] {
            shell.key(KeyCode::F1, state, false, &ctx);
        }
        shell.key(KeyCode::ControlLeft, Pressed, false, &ctx);
        for key in [KeyCode::KeyN, KeyCode::KeyO, KeyCode::Comma] {
            for state in [Pressed, Released] {
                shell.key(key, state, false, &ctx);
            }
        }
        shell.key(KeyCode::ControlLeft, Released, false, &ctx);
        assert!(shell.dialog.is_none());
        assert!(shell.consumed.is_empty());
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            10
        );
    }

    fn button(button: winit::event::MouseButton, state: ElementState) -> WindowEvent {
        WindowEvent::MouseInput {
            device_id: winit::event::DeviceId::dummy(),
            button,
            state,
        }
    }

    #[test]
    fn captured_pointer_filter_blocks_egui_but_retains_window_lifecycle() {
        let mut events = Events {
            captured: true,
            ..Default::default()
        };
        for event in [
            button(winit::event::MouseButton::Left, Pressed),
            button(winit::event::MouseButton::Right, Released),
            WindowEvent::CursorMoved {
                device_id: winit::event::DeviceId::dummy(),
                position: winit::dpi::PhysicalPosition::new(500.0, 10.0),
            },
            WindowEvent::MouseWheel {
                device_id: winit::event::DeviceId::dummy(),
                delta: winit::event::MouseScrollDelta::LineDelta(1.0, 1.0),
                phase: winit::event::TouchPhase::Moved,
            },
            WindowEvent::CursorLeft {
                device_id: winit::event::DeviceId::dummy(),
            },
        ] {
            assert!(!events.forward(&event), "{event:?}");
        }
        assert_eq!(events.queue.len(), 4);
        assert!(events.forward(&WindowEvent::CloseRequested));
        assert!(events.forward(&WindowEvent::Occluded(false)));
        events.captured = false;
        assert!(events.forward(&button(winit::event::MouseButton::Left, Pressed)));
    }

    fn captured_shell() -> Shell {
        let mut shell = shell();
        let mut presenter = crate::tests::mouse_presenter(true);
        presenter.mouse.pointer_inside(true);
        presenter.mouse_button(winit::event::MouseButton::Left, Pressed);
        presenter.mouse_button(winit::event::MouseButton::Left, Released);
        assert!(presenter.mouse.captured());
        shell.presenter = Some(presenter);
        shell.sync_routing();
        shell
    }

    #[test]
    fn captured_clicks_outside_screen_reach_guest_and_cursor_stays_hidden() {
        let mut shell = captured_shell();
        let ctx = egui::Context::default();
        shell.screen = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), Vec2::splat(100.0));
        shell.pointer = egui::pos2(5.0, 5.0);
        shell
            .events
            .borrow_mut()
            .forward(&button(winit::event::MouseButton::Right, Pressed));
        shell.input(&ctx, false);
        let presenter = shell.presenter.as_mut().unwrap();
        assert!(presenter.mouse.captured());
        let delivered = presenter.machine_mut().deliver_pointer_for_test();
        assert_eq!(
            delivered
                .iter()
                .map(|input| input.buttons)
                .collect::<Vec<_>>(),
            [2]
        );
        let mut output = ctx.run_pass(Default::default(), |ctx| shell.cursor(ctx));
        output.textures_delta.clear();
        assert_eq!(output.platform_output.cursor_icon, egui::CursorIcon::None);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn pointer_release_and_pause_preserve_guest_keyboard_focus() {
        let mut shell = captured_shell();
        let ctx = egui::Context::default();
        shell.command(Command::Release, &ctx);
        assert!(shell.guest_focus);
        assert!(!shell.presenter.as_ref().unwrap().mouse.captured());
        shell.key(KeyCode::F1, Pressed, false, &ctx);
        shell.key(KeyCode::F1, Released, false, &ctx);
        assert!(shell.dialog.is_none());
        shell.key(KeyCode::KeyA, Pressed, false, &ctx);
        shell.command(Command::Pause, &ctx);
        assert!(shell.guest_focus);
        shell.key(KeyCode::KeyA, Released, false, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            4
        );
        shell.command(Command::Pause, &ctx);
        shell.key(KeyCode::KeyB, Pressed, false, &ctx);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            5
        );
    }

    #[test]
    fn uncovering_never_overrides_user_or_dialog_pause() {
        use std::time::Duration;
        for user_pause in [true, false] {
            let mut shell = shell();
            let ctx = egui::Context::default();
            shell.paused = user_pause;
            if !user_pause {
                shell.dialog = Some(("Settings", "not yet available".into()));
            }
            shell.tick(&ctx, false, Duration::from_millis(10));
            shell
                .events
                .borrow_mut()
                .window(WindowEvent::Occluded(true));
            shell
                .events
                .borrow_mut()
                .window(WindowEvent::Occluded(false));
            shell.input(&ctx, false);
            assert_eq!(
                shell.presenter.as_ref().unwrap().occluded_since,
                Some(Duration::from_millis(10))
            );
            shell.tick(&ctx, false, Duration::from_millis(20));
            assert!(shell.modal_paused);
            assert_eq!(
                shell
                    .presenter
                    .as_ref()
                    .unwrap()
                    .assembled_ref()
                    .machine
                    .virtual_time(),
                0
            );
            shell.paused = false;
            shell.dialog = None;
            shell.tick(&ctx, false, Duration::from_millis(30));
            assert!(!shell.modal_paused);
            assert!(shell.presenter.as_ref().unwrap().occluded_since.is_none());
        }
    }

    #[test]
    fn minimize_restore_without_occlusion_events_discards_hidden_time() {
        use std::time::Duration;
        let mut shell = shell();
        shell.presenter.as_mut().unwrap().full_slices = true;
        let ctx = egui::Context::default();
        shell.visibility(&ctx, true);
        shell.tick(&ctx, false, Duration::from_millis(100));
        assert!(shell.modal_paused);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled_ref()
                .machine
                .virtual_time(),
            0
        );
        shell.visibility(&ctx, false);
        shell.tick(&ctx, false, Duration::from_millis(200));
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(presenter.occluded_since.is_none());
        assert_eq!(presenter.pacing.dropped, 0);
        assert_eq!(presenter.assembled_ref().machine.virtual_time(), 0);
        shell.tick(&ctx, false, Duration::from_millis(220));
        assert!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled_ref()
                .machine
                .virtual_time()
                > 0
        );
    }

    #[test]
    fn minimized_focus_loss_keeps_host_modifiers_and_guest_releases_consistent() {
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyP] {
            shell.key(key, Pressed, false, &ctx);
        }
        shell
            .events
            .borrow_mut()
            .window(WindowEvent::Focused(false));
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyP] {
            shell.events.borrow_mut().key(key, Released);
        }
        shell.visibility(&ctx, true);
        assert!(shell.events.borrow().queue.is_empty());
        assert_eq!(shell.modifiers, [false; 8]);
        assert!(shell.held.is_empty());
        assert!(shell.consumed.is_empty());
        assert_eq!(take_guest_scancodes(&mut shell), [0x1D, 0x38, 0xB8, 0x9D]);
        shell.visibility(&ctx, false);
        shell.paused = false;
        shell.focus_guest();
        shell.key(KeyCode::F1, Pressed, false, &ctx);
        assert!(shell.dialog.is_none());
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            1
        );
    }

    fn journal_shell(temp: &machine_file::test_support::TempDir, program: Vec<u8>) -> Shell {
        journal_shell_with_args(temp, program, crate::parse_args([]).unwrap())
    }

    fn journal_shell_with_args(
        temp: &machine_file::test_support::TempDir,
        program: Vec<u8>,
        args: crate::Args,
    ) -> Shell {
        let base = temp.path().join("base.img");
        let journal = temp.path().join("disk.upfj");
        std::fs::write(&base, vec![0; 1_474_560]).unwrap();
        let mut config = assembly::bios(vec![0xf4; 32768], Some(vec![0; 1_474_560]));
        config.board = assembly::BoardModel::Bare;
        config.devices.retain(|device| {
            !matches!(
                device.kind,
                assembly::DeviceKind::Keyboard(_) | assembly::DeviceKind::SysCtrlPortA
            )
        });
        let demo = assembly::at_boot(program);
        config.cpu = demo.cpu;
        config.images = demo.images;
        config.drives[0].image = assembly::ImageSource::File(base);
        config.drives[0].write = assembly::WriteMode::journal(&journal);
        let presenter = Presenter::new(
            &config,
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap();
        let mut shell = shell();
        shell.args = args;
        // A CPU refusal saves a report beside the machine, even with synthetic firmware.
        shell.args.machine = temp.path().join("machine.toml");
        shell.presenter = Some(presenter);
        shell
    }

    fn machine_source_files(
        root: &std::path::Path,
    ) -> BTreeMap<std::path::PathBuf, Option<Vec<u8>>> {
        fn visit(
            path: &std::path::Path,
            files: &mut BTreeMap<std::path::PathBuf, Option<Vec<u8>>>,
        ) {
            if path.is_dir() {
                files.insert(path.to_owned(), None);
                for entry in std::fs::read_dir(path).unwrap() {
                    visit(&entry.unwrap().path(), files);
                }
            } else if path.is_file() {
                files.insert(path.to_owned(), Some(std::fs::read(path).unwrap()));
            }
        }
        let mut files = BTreeMap::new();
        visit(root, &mut files);
        files
    }

    #[test]
    fn stop_preserves_unsealed_execution_error_journals() {
        let temp = machine_file::test_support::TempDir::new("shell-execution-error");
        let source_root = machine_file::test_support::TempDir::new("shell-source-machine");
        let source = source_root.path().join("at-freedos.toml");
        std::fs::write(&source, b"schema=1\n").unwrap();
        let source_before = machine_source_files(source_root.path());
        let args =
            crate::parse_args(["--machine".into(), source.clone().into_os_string()]).unwrap();
        let mut shell = journal_shell_with_args(&temp, vec![0x0f, 0x05], args);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert!(shell.presenter.as_ref().unwrap().error.is_some());
        assert!(!shell.stop());
        assert!(shell.presenter.is_none());
        assert!(shell.errors.borrow().is_some());
        assert_eq!(machine_source_files(source_root.path()), source_before);
        let report = temp
            .path()
            .join("machine.toml.local/diagnostics")
            .join(CPU_STOP_REPORT_FILE);
        assert!(std::fs::read_to_string(report)
            .unwrap()
            .starts_with("cpu stop report\n"));
        assert_eq!(
            std::fs::metadata(temp.path().join("disk.upfj"))
                .unwrap()
                .len(),
            128
        );
    }

    #[test]
    fn stop_finishes_the_session_a_halt_left_open() {
        let temp = machine_file::test_support::TempDir::new("shell-finished-halt");
        let mut shell = journal_shell(&temp, vec![0xfa, 0xf4]);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert!(!shell.presenter.as_ref().unwrap().running);
        assert!(shell.presenter.as_ref().unwrap().halted);
        assert!(shell.presenter.as_ref().unwrap().error.is_none());
        let journal = temp.path().join("disk.upfj");
        // The halted guest can still be reset, so its journal stays open.
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 128);
        assert!(shell.stop());
        assert!(shell.errors.borrow().is_none());
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 128 + 640);
    }

    /// A session of the open ring ROM, whose guest takes 12,000 software
    /// interrupts that never fold, run to its halt; the retired count at
    /// the halt.
    fn ring_session(shell: &mut Shell, ctx: &egui::Context) -> u64 {
        use std::time::Duration;
        let mut presenter = Presenter::new(
            &assembly::bios(assembly::test_support::delivery_ring_rom(), None),
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
            &mut io::sink(),
        )
        .unwrap();
        presenter.full_slices = true;
        shell.started(presenter);
        for frame in 1..=50 {
            shell.tick(ctx, false, Duration::from_millis(20 * frame));
            if shell.presenter.as_ref().unwrap().halted {
                break;
            }
        }
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(presenter.halted);
        cpu::Cpu::retired(presenter.assembled_ref().machine.cpu())
    }

    /// The recording file's lines: the two header lines, checked, then the
    /// entries and the summary.
    fn recording_lines(path: &std::path::Path, retired: u64) -> Vec<String> {
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_owned).collect();
        assert_eq!(
            lines[0],
            format!(
                "trace    exceptions and interrupts the guest took from retired 0 to retired {retired}"
            )
        );
        assert!(
            lines[1].starts_with("trace    the last 10000 entries, oldest first"),
            "{}",
            lines[1]
        );
        lines[2..].to_vec()
    }

    /// Record exceptions keeps the last 10,000 entries of a guest that
    /// takes 12,000 deliveries, with exact totals, and writes them to one
    /// `trace-N.txt` per recording at Save machine report (whose report
    /// names the file), at Stop, at unchecking and when the shell closes.
    /// It is off at launch; on or off, the guest halts at the same retired
    /// count.
    #[test]
    fn record_exceptions_writes_the_last_entries_at_every_trigger() {
        let temp = machine_file::test_support::TempDir::new("shell-record-exceptions");
        let folder = temp.path().join("unemu").join("diagnostics");
        let ctx = egui::Context::default();
        let trace = |n: u32| folder.join(format!("trace-{n}.txt"));
        let check = |lines: &[String]| {
            assert_eq!(lines.len(), 10_000 + 3);
            for (index, line) in lines[..10_000].iter().enumerate() {
                let n = 2_000 + index as u64;
                let (vector, ip) = if n.is_multiple_of(2) {
                    (0x60, 0x27)
                } else {
                    (0x61, 0x29)
                };
                let retired = 12 + 5 * (n / 2) + 2 * (n % 2);
                assert!(
                    line.starts_with(&format!(
                        "delivery {retired} software INT {vector:02X}h vector {vector:02x} at f800:{ip:04x} "
                    )),
                    "{index}: {line}"
                );
            }
            assert_eq!(
                lines[10_000..],
                [
                    "summary  12000 deliveries, 10000 of them in the entries kept, 0 dropped before they were read",
                    "summary  INT 60h: 6000",
                    "summary  INT 61h: 6000",
                ]
            );
        };

        let mut shell = shell();
        shell.presenter = None;
        shell.args.diagnostics = Ok(folder.clone());
        assert!(!shell.record_exceptions, "off at launch");
        let plain = ring_session(&mut shell, &ctx);
        assert!(shell.stop());
        assert!(!folder.exists(), "nothing recorded");

        // Save machine report writes the recording so far; Stop rewrites
        // the same file.
        shell.set_record_exceptions(true);
        assert_eq!(ring_session(&mut shell, &ctx), plain);
        shell.save_diagnostics();
        assert_eq!(
            shell.message,
            format!(
                "Machine report saved to {}; exceptions to {}",
                folder.join("diag-1.txt").display(),
                trace(1).display()
            )
        );
        let report = std::fs::read_to_string(folder.join("diag-1.txt")).unwrap();
        assert!(
            report.ends_with(&format!(
                "\ndesktop  exceptions recorded to {}\n",
                trace(1).display()
            )),
            "{report}"
        );
        assert!(
            report.contains(
                "\ndelivery the last 64 of 12000 exceptions and interrupts in 64 entries"
            ),
            "{report}"
        );
        check(&recording_lines(&trace(1), plain));
        assert!(shell.stop());
        assert_eq!(
            shell.message,
            format!("Ready — Exceptions recorded to {}", trace(1).display())
        );
        check(&recording_lines(&trace(1), plain));

        // Stop alone writes the next session's own file.
        ring_session(&mut shell, &ctx);
        assert!(shell.stop());
        check(&recording_lines(&trace(2), plain));

        // Unchecking writes and ends the recording: Stop writes no more.
        ring_session(&mut shell, &ctx);
        shell.set_record_exceptions(false);
        assert_eq!(
            shell.message,
            format!("Exceptions recorded to {}", trace(3).display())
        );
        check(&recording_lines(&trace(3), plain));
        assert!(shell.stop());
        assert!(!trace(4).exists());

        // Checking it while a machine runs records from that moment; closing
        // the window writes it.
        ring_session(&mut shell, &ctx);
        shell.set_record_exceptions(true);
        drop(shell);
        let lines = std::fs::read_to_string(trace(4)).unwrap();
        assert!(lines.starts_with(&format!(
            "trace    exceptions and interrupts the guest took from retired {plain} to retired {plain}\n"
        )), "{lines}");
        assert!(lines.ends_with(
            "summary  0 deliveries, 0 of them in the entries kept, 0 dropped before they were read\n"
        ), "{lines}");
    }

    /// With Record exceptions on but no recording running (its start was
    /// refused), the report says why instead of calling the item off.
    #[test]
    fn the_report_says_why_a_checked_recording_is_missing() {
        let temp = machine_file::test_support::TempDir::new("shell-recording-refused");
        let folder = temp.path().join("unemu").join("diagnostics");
        let mut shell = keyboard_shell();
        shell.args.diagnostics = Ok(folder.clone());
        shell.record_exceptions = true;
        shell.presenter.as_mut().unwrap().recording_error = Some("refused here".into());
        shell.save_diagnostics();
        let report = std::fs::read_to_string(folder.join("diag-1.txt")).unwrap();
        assert!(
            report.ends_with(
                "\ndesktop  exceptions not recorded: Record exceptions is on, but the recording did not start (refused here)\n"
            ),
            "{report}"
        );
    }

    /// The accessibility nodes of one pass of the menu bar: each labelled
    /// node with whether it is disabled and whether it is checked.
    fn menu_nodes(ctx: &egui::Context, shell: &mut Shell) -> Vec<(String, bool, Option<bool>)> {
        ctx.enable_accesskit();
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1040.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| shell.menus(ui, &mut commands),
        );
        output.textures_delta.clear();
        let nodes = output
            .platform_output
            .accesskit_update
            .take()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| {
                        Some((
                            node.label()?.to_owned(),
                            node.is_disabled(),
                            node.toggled()
                                .map(|toggled| toggled == egui::accesskit::Toggled::True),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        output.drop_without_applying_deltas();
        nodes
    }

    /// Open Tools › Diagnostics and leave it open; what it draws.
    fn open_diagnostics_menu(ctx: &egui::Context, shell: &mut Shell) -> Drawn {
        egui::Popup::close_all(ctx);
        let mut drawn = draw(ctx, shell, Vec::new());
        click(ctx, shell, label_rect(&drawn.text, "Tools").center());
        drawn = draw(ctx, shell, Vec::new());
        let diagnostics = label_rect(&drawn.text, "Diagnostics").center();
        click(ctx, shell, diagnostics);
        for _ in 0..2 {
            drawn = draw(ctx, shell, vec![egui::Event::PointerMoved(diagnostics)]);
        }
        drawn
    }

    /// Tools › Diagnostics holds Save machine report with its chord,
    /// Record exceptions as a check item and Open diagnostics folder. Save
    /// is disabled without a machine, saying why; Open diagnostics folder
    /// always works. Each item does what it says.
    #[test]
    fn the_tools_menu_has_the_diagnostics_items() {
        let temp = machine_file::test_support::TempDir::new("shell-diagnostics-menu");
        let folder = temp.path().join("unemu").join("diagnostics");
        let ctx = egui::Context::default();
        let item = |nodes: &[(String, bool, Option<bool>)], label: &str| {
            nodes
                .iter()
                .find(|(shown, ..)| {
                    shown
                        .split_once(" Ctrl")
                        .map_or(shown.as_str(), |(item, _)| item)
                        .trim_end_matches(" ⌘⇧D")
                        == label
                })
                .map(|(_, disabled, checked)| (*disabled, *checked))
                .unwrap_or_else(|| panic!("{label} not in {nodes:?}"))
        };
        for macos in [false, true] {
            let mut stopped = shell();
            stopped.presenter = None;
            stopped.macos_shortcuts = macos;
            stopped.args.diagnostics = Ok(folder.clone());
            let drawn = open_diagnostics_menu(&ctx, &mut stopped);
            assert!(shows(
                &drawn.text,
                if macos { "⌘⇧D" } else { "Ctrl+Alt+Shift+D" }
            ));
            let nodes = menu_nodes(&ctx, &mut stopped);
            assert_eq!(item(&nodes, "Save machine report"), (true, None));
            assert_eq!(item(&nodes, "Record exceptions"), (false, Some(false)));
            assert_eq!(item(&nodes, "Open diagnostics folder"), (false, None));
        }

        // The disabled item says why on hover.
        let mut stopped = shell();
        stopped.presenter = None;
        assert_eq!(
            stopped.save_report_disabled(),
            Some("Start a machine first")
        );

        // Open diagnostics folder creates the folder and shows it.
        let drawn = open_diagnostics_menu(&ctx, &mut stopped);
        stopped.args.diagnostics = Ok(folder.clone());
        crate::diagnostics::REVEALED.with(|revealed| revealed.borrow_mut().clear());
        click(
            &ctx,
            &mut stopped,
            label_rect(&drawn.text, "Open diagnostics folder").center(),
        );
        assert!(folder.is_dir());
        assert_eq!(
            crate::diagnostics::REVEALED.with(|revealed| revealed.borrow().clone()),
            vec![folder.clone()]
        );
        assert_eq!(
            stopped.message,
            format!("Diagnostics folder: {}", folder.display())
        );

        // With a machine running Save is enabled and writes the report;
        // Record exceptions checks and unchecks.
        let mut running = keyboard_shell();
        running.args.diagnostics = Ok(folder.clone());
        let drawn = open_diagnostics_menu(&ctx, &mut running);
        let nodes = menu_nodes(&ctx, &mut running);
        assert_eq!(item(&nodes, "Save machine report"), (false, None));
        click(
            &ctx,
            &mut running,
            label_rect(&drawn.text, "Save machine report").center(),
        );
        assert_eq!(
            running.message,
            format!(
                "Machine report saved to {}",
                folder.join("diag-1.txt").display()
            )
        );
        let drawn = open_diagnostics_menu(&ctx, &mut running);
        click(
            &ctx,
            &mut running,
            label_rect(&drawn.text, "Record exceptions").center(),
        );
        assert!(running.record_exceptions);
        open_diagnostics_menu(&ctx, &mut running);
        let nodes = menu_nodes(&ctx, &mut running);
        assert_eq!(item(&nodes, "Record exceptions"), (false, Some(true)));
        egui::Popup::close_all(&ctx);
    }

    #[test]
    fn a_halted_session_takes_floppies_and_ctrl_alt_del_restarts_it() {
        use std::time::Duration;
        let ctx = egui::Context::default();
        let mut shell = shell();
        let mut presenter = crate::tests::counting_halt_presenter();
        presenter.full_slices = true;
        shell.started(presenter);
        shell.tick(&ctx, false, Duration::from_millis(20));
        assert!(shell.presenter.as_ref().unwrap().halted);
        assert!(shell.paused);
        assert_eq!(
            shell.message,
            "Machine halted — insert a boot disk and press Ctrl+Alt+Del, or Stop to return to the library"
        );
        for command in [
            Command::Insert(0),
            Command::InsertFile(0),
            Command::Next(0),
            Command::Eject(0),
            Command::Reset,
        ] {
            assert!(shell.command_allowed(command), "{command:?}");
        }
        // Resume would only hide that the guest waits for a reset.
        assert!(!shell.command_allowed(Command::Pause));
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().select(Some(1));
        shell.confirm_insert();
        shell.tick(&ctx, false, Duration::from_millis(40));
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "two.img"
        );
        assert!(presenter.halted);
        let time = presenter.assembled().machine.virtual_time();
        shell.command(Command::Reset, &ctx);
        let presenter = shell.presenter.as_ref().unwrap();
        // The reset button, not keystrokes the halted guest cannot read.
        assert_eq!(presenter.keyboard().pending_events(), 0);
        assert!(presenter.running);
        assert_eq!(presenter.machine().cpu_resets(), 1);
        assert_eq!(presenter.assembled_ref().machine.virtual_time(), time);
        assert!(!shell.paused);
        for elapsed in (60..=2000).step_by(20) {
            shell.tick(&ctx, false, Duration::from_millis(elapsed));
            if shell.presenter.as_ref().unwrap().halted {
                break;
            }
        }
        let presenter = shell.presenter.as_mut().unwrap();
        assert!(presenter.halted, "the firmware ran again and halted");
        assert_eq!(crate::tests::starts_counted(presenter), 2);
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "two.img"
        );
        // A halted session still ends without the stop prompt.
        assert!(!shell.needs_stop_prompt());
        assert!(shell.stop());
    }

    fn finish_readiness(shell: &mut Shell) {
        shell.poll_readiness(&egui::Context::default());
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        shell.apply_readiness(job.index, job.revision, prepared);
    }

    /// A stopped shell with one ready machine file selected: its 1.44M A:
    /// starts empty with `boot.img` in its catalogue, and its B: is empty.
    fn stopped_machine_file_shell(
        temp: &machine_file::test_support::TempDir,
    ) -> (Shell, std::path::PathBuf) {
        std::fs::write(temp.path().join("bios.bin"), vec![0xF4; 32768]).unwrap();
        std::fs::write(temp.path().join("boot.img"), vec![0; 1_474_560]).unwrap();
        let path = temp.path().join("machine.toml");
        std::fs::write(&path, "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='mda'\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.44M'\nempty=true\n[[card.drives.media]]\nimage='boot.img'\nwrite='readonly'\n[[card.drives]]\nunit=1\ntype='1.44M'\nempty=true\n").unwrap();
        let mut shell = shell();
        shell.presenter = None;
        shell.args.options.allow_unlisted_firmware = true;
        // A store path that does not exist: the user's own store is never read.
        shell.preferences.store_root = Some(temp.path().join("no-store"));
        shell.library = vec![LibraryEntry::inspect_with_rom_dirs(
            path.clone(),
            shell.args.options,
            &[],
        )];
        shell.selected = Some(0);
        shell.refresh_library();
        finish_readiness(&mut shell);
        shell.sync_pending_media();
        (shell, path)
    }

    #[test]
    fn floppies_inserted_before_start_go_in_before_the_first_instruction() {
        let temp = machine_file::test_support::TempDir::new("shell-pending-floppy");
        let (mut shell, path) = stopped_machine_file_shell(&temp);
        let machine_file = std::fs::read(&path).unwrap();
        let ctx = egui::Context::default();
        for command in [
            Command::Insert(0),
            Command::Insert(1),
            Command::InsertFile(0),
        ] {
            assert!(shell.command_allowed(command), "{command:?}");
        }
        for command in [
            Command::Eject(0),
            Command::Next(0),
            Command::WriteMode(0),
            Command::InsertSet(0),
        ] {
            assert!(!shell.command_allowed(command), "{command:?}");
        }
        assert!(shell.drive_views().is_empty());
        // A set chosen in the media picker journals into the machine file's
        // local media folder, as it does once the machine runs.
        shell.command(Command::Insert(0), &ctx);
        assert!(shell.picker.is_some());
        shell.apply_picker(crate::media_picker::tests::set_outcome(0));
        let library = shell.insert_dialog.as_ref().unwrap().library.as_ref();
        assert_eq!(
            library.map(|library| (library.workspace.clone(), library.journal)),
            Some((
                image_store::location::machine_media_dir(&path).unwrap(),
                true
            ))
        );
        shell.insert_dialog = None;
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        assert_eq!(draft.slot.entries.len(), 1, "the machine file's catalogue");
        draft.select(Some(0));
        shell.confirm_insert();
        assert!(shell.insert_dialog.is_none());
        assert_eq!(
            shell.message,
            "A: boot.img is in the drive when Start boots"
        );
        let other = temp.path().join("other.img");
        std::fs::write(&other, vec![0; 737_280]).unwrap();
        shell.command(Command::InsertFile(1), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        let browsed = crate::media_dialog::browse(&other, draft.slot.geometry).unwrap();
        draft.add_browsed(browsed);
        shell.confirm_insert();
        let chips: Vec<_> = shell
            .drive_views()
            .iter()
            .map(crate::drive_chips::DriveView::chip_text)
            .collect();
        assert_eq!(
            chips,
            [
                "A: boot.img · 1.44M · read-only · at Start",
                "B: other.img · 720K · read-only · at Start"
            ]
        );
        assert!(shell.command_allowed(Command::Eject(1)));
        shell.command(Command::Eject(1), &ctx);
        assert_eq!(shell.drive_views().len(), 1);
        assert!(!shell.command_allowed(Command::Eject(1)));
        shell.command(Command::Start, &ctx);
        finish_readiness(&mut shell);
        assert!(shell.pending_media.is_none());
        let presenter = shell.presenter.as_mut().unwrap();
        // Queued before the first instruction: the first run boundary puts
        // the floppy in before POST reads the drive.
        assert_eq!(presenter.assembled().machine.virtual_time(), 0);
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(0)
            )]
        );
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "boot.img"
        );
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 1),
            "empty"
        );
        assert_eq!(std::fs::read(&path).unwrap(), machine_file);
        assert!(shell.stop());
    }

    #[test]
    fn another_selection_drops_the_floppies_inserted_for_start() {
        let temp = machine_file::test_support::TempDir::new("shell-pending-selection");
        let (mut shell, _) = stopped_machine_file_shell(&temp);
        let ctx = egui::Context::default();
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().select(Some(0));
        shell.confirm_insert();
        assert_eq!(shell.drive_views().len(), 1);
        // A draft of this machine's catalogue closes with the selection.
        shell.command(Command::InsertFile(0), &ctx);
        assert!(shell.insert_dialog.is_some());
        shell.selected = None;
        shell.sync_pending_media();
        assert!(shell.pending_media.is_none());
        assert!(shell.insert_dialog.is_none());
        assert!(!shell.command_allowed(Command::Insert(0)));
        shell.selected = Some(0);
        shell.sync_pending_media();
        assert!(shell.command_allowed(Command::Insert(0)));
        assert!(shell.drive_views().is_empty());
        assert!(!shell.command_allowed(Command::Eject(0)));
    }

    #[test]
    fn a_floppy_inserted_before_an_image_starts_joins_its_catalogue() {
        use crate::library_test_support::Fixture;
        let f = Fixture::new();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.stop();
        shell.args.options.allow_unlisted_firmware = true;
        shell.preferences.store_root = Some(f.temp.path().join("no-store"));
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell.image_library.selected = Some(f.row.clone());
        shell.image_library.recheck();
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.image_library.readiness.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(shell.image_library.ready());
        shell.sync_pending_media();
        assert!(shell.command_allowed(Command::Insert(0)));
        // The image's workspace opens only at Start, so library disks chosen
        // before it go in write-protected.
        shell.apply_picker(crate::media_picker::tests::set_outcome(0));
        let library = shell.insert_dialog.as_ref().unwrap().library.as_ref();
        assert!(library
            .is_some_and(|library| !library.journal && library.journal_unavailable.is_some()));
        shell.insert_dialog = None;
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        assert!(
            draft.slot.entries.is_empty(),
            "the image's disks join at Start"
        );
        let extra = f.temp.path().join("extra.img");
        std::fs::write(&extra, vec![7; 360 * 1024]).unwrap();
        let browsed = crate::media_dialog::browse(&extra, draft.slot.geometry).unwrap();
        draft.add_browsed(browsed);
        shell.confirm_insert();
        let chips: Vec<_> = shell
            .drive_views()
            .iter()
            .map(crate::drive_chips::DriveView::chip_text)
            .collect();
        assert_eq!(chips, ["A: extra.img · 360K · read-only · at Start"]);
        shell.command(Command::Start, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.presenter.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let presenter = shell.presenter.as_mut().expect("the image started");
        // Appended after the image's own two disks.
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(2)
            )]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "extra.img"
        );
        assert!(shell.stop());
        shell.image_library.shutdown();
    }

    #[test]
    fn an_error_dialog_on_a_halted_session_still_lets_stop_finish_it() {
        let temp = machine_file::test_support::TempDir::new("shell-halt-error");
        let mut shell = journal_shell(&temp, vec![0xfa, 0xf4]);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert!(shell.presenter.as_ref().unwrap().halted);
        shell.show_error("Image store worker stopped".into());
        // The error ends the session's input; Ctrl+Alt+Del is no reset now.
        assert!(!shell.command_allowed(Command::Insert(0)));
        assert!(!shell.presenter.as_ref().unwrap().awaits_reset());
        let journal = temp.path().join("disk.upfj");
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 128);
        assert!(shell.stop());
        assert_eq!(std::fs::metadata(&journal).unwrap().len(), 128 + 640);
    }

    #[test]
    fn exit_from_a_halted_session_finishes_its_cmos_journal_as_the_cli_does() {
        let temp = machine_file::test_support::TempDir::new("shell-halt-exit");
        let base = temp.path().join("cmos.bin");
        std::fs::write(&base, [0; 64]).unwrap();
        let cli = temp.path().join("cli.upfj");
        let mut reference =
            machine_file::assemble(&crate::tests::cmos_halt_config(&base, &cli)).unwrap();
        reference
            .run_session(&assembly::RunPolicy::natural(Some(200), None))
            .unwrap();
        reference.finish_persistence().unwrap();
        drop(reference);
        let window = temp.path().join("window.upfj");
        let mut shell = shell();
        let mut presenter =
            crate::tests::vga_presenter(&crate::tests::cmos_halt_config(&base, &window));
        presenter
            .advance_fully(std::time::Duration::from_millis(20))
            .unwrap();
        assert!(presenter.halted);
        shell.presenter = Some(presenter);
        let ctx = egui::Context::default();
        let (commands, _) = pass(&ctx, Default::default(), |ctx| {
            shell.command(Command::Exit, ctx)
        });
        assert!(commands.contains(&egui::ViewportCommand::Close));
        assert!(shell.presenter.is_none());
        assert_eq!(std::fs::read(window).unwrap(), std::fs::read(cli).unwrap());
    }

    #[test]
    fn ctrl_alt_end_resets_a_halted_machine_and_drops_keys_held_at_the_halt() {
        use std::time::Duration;
        let ctx = egui::Context::default();
        let mut shell = shell();
        let mut presenter = crate::tests::counting_halt_presenter();
        presenter.full_slices = true;
        shell.started(presenter);
        // Shift goes down while the guest runs and comes up while it is
        // halted, when no key reaches the guest.
        shell.key(KeyCode::ShiftLeft, Pressed, false, &ctx);
        shell.tick(&ctx, false, Duration::from_millis(20));
        assert!(shell.presenter.as_ref().unwrap().awaits_reset());
        shell.key(KeyCode::ShiftLeft, Released, false, &ctx);
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::End] {
            shell.key(key, Pressed, false, &ctx);
        }
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(presenter.running);
        assert_eq!(presenter.machine().cpu_resets(), 1);
        for key in [KeyCode::End, KeyCode::AltLeft, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        // Neither the chord nor the Shift held at the halt reaches the new
        // boot, not even as the break a later focus loss sends for held keys.
        let presenter = shell.presenter.as_mut().unwrap();
        presenter.focus_changed(false);
        assert_eq!(presenter.keyboard().pending_events(), 0);
        assert!(shell.stop());
    }

    #[test]
    fn start_leaves_out_a_changed_pending_floppy_and_inserts_the_others() {
        let temp = machine_file::test_support::TempDir::new("shell-pending-failure");
        let (mut shell, _) = stopped_machine_file_shell(&temp);
        let ctx = egui::Context::default();
        for (unit, name) in [(0, "a.img"), (1, "b.img")] {
            let path = temp.path().join(name);
            std::fs::write(&path, vec![0; 1_474_560]).unwrap();
            shell.command(Command::InsertFile(unit), &ctx);
            let draft = shell.insert_dialog.as_mut().unwrap();
            let browsed = crate::media_dialog::browse(&path, draft.slot.geometry).unwrap();
            draft.add_browsed(browsed);
            shell.confirm_insert();
        }
        // A: changes after its Insert pinned its SHA-256.
        std::fs::write(temp.path().join("a.img"), vec![1; 1_474_560]).unwrap();
        shell.command(Command::Start, &ctx);
        finish_readiness(&mut shell);
        let (title, text) = shell.dialog.clone().expect("the left-out floppy is named");
        assert_eq!(title, "Insert floppy");
        assert!(text.contains("A: a.img: "), "{text}");
        assert!(!text.contains("B:"), "{text}");
        shell.dialog = None;
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "empty"
        );
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 1),
            "b.img"
        );
        assert!(shell.stop());
    }

    #[test]
    fn a_changed_machine_file_never_swaps_the_disk_inserted_before_start() {
        let temp = machine_file::test_support::TempDir::new("shell-pending-changed");
        let other = temp.path().join("other.img");
        std::fs::write(&other, vec![0; 1_474_560]).unwrap();
        let ctx = egui::Context::default();
        let insert_boot = |shell: &mut Shell| {
            shell.command(Command::InsertFile(0), &ctx);
            let draft = shell.insert_dialog.as_mut().unwrap();
            assert_eq!(draft.slot.entries[0].medium.name, "boot.img");
            draft.select(Some(0));
            shell.confirm_insert();
            assert_eq!(shell.drive_views().len(), 1);
        };
        // The same catalogue place now lists another disk.
        let resave = |path: &std::path::Path| {
            let text = std::fs::read_to_string(path).unwrap();
            std::fs::write(path, text.replace("image='boot.img'", "image='other.img'")).unwrap();
        };
        // Saved again from Settings: the library reads the file again.
        let (mut shell, path) = stopped_machine_file_shell(&temp);
        insert_boot(&mut shell);
        resave(&path);
        shell.refresh_library();
        assert!(shell.pending_media.is_none());
        assert_eq!(
            shell.message,
            "Floppies inserted for Start were taken out: the machine was read again"
        );
        finish_readiness(&mut shell);
        shell.sync_pending_media();
        assert!(shell.drive_views().is_empty());
        // Edited outside the program: Start reads the new file and leaves the
        // pending disk out instead of inserting another one.
        let temp = machine_file::test_support::TempDir::new("shell-pending-edited");
        std::fs::write(temp.path().join("other.img"), vec![0; 1_474_560]).unwrap();
        let (mut shell, path) = stopped_machine_file_shell(&temp);
        insert_boot(&mut shell);
        resave(&path);
        shell.command(Command::Start, &ctx);
        finish_readiness(&mut shell);
        let (_, text) = shell.dialog.clone().expect("the left-out floppy is named");
        assert!(
            text.contains("A: boot.img: the machine file now lists another disk there"),
            "{text}"
        );
        shell.dialog = None;
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        assert_eq!(
            shell
                .presenter
                .as_mut()
                .unwrap()
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "empty"
        );
        assert!(shell.stop());
    }

    /// A machine file for the open AT BIOS with a 1.2M A: and the FreeDOS
    /// 1.2M boot disk in its catalogue; `None` without fixtures. With
    /// `no_boot`, A: starts with a disk whose boot sector gives up through
    /// INT 18h, as a hard disk's master boot record does without an active
    /// partition; otherwise A: starts empty.
    fn open_bios_floppy_machine(
        temp: &machine_file::test_support::TempDir,
        no_boot: bool,
    ) -> Option<std::path::PathBuf> {
        let root = pc_harness::test_support::root();
        let bios = root.join("vendor/unemu-at-bios/bios-unemu-at.bin");
        let floppy = root.join("vendor/freedos/freedos-1200.img");
        for (path, fetch) in [
            (&bios, "vendor/unemu-at-bios/fetch.sh"),
            (&floppy, "vendor/freedos/fetch.sh"),
        ] {
            if !path.exists() {
                let message = format!("missing fixture {} — run {fetch}", path.display());
                assert!(
                    std::env::var_os("UNEMUPC_REQUIRE_FIXTURES").as_deref()
                        != Some(std::ffi::OsStr::new("1")),
                    "{message}"
                );
                eprintln!("skipping: {message}");
                return None;
            }
        }
        let quoted = |path: &std::path::Path| {
            toml::Value::String(path.to_string_lossy().into_owned()).to_string()
        };
        let first = if no_boot {
            let mut disk = vec![0; 1_228_800];
            disk[..2].copy_from_slice(&[0xCD, 0x18]); // int 18h
            disk[510..512].copy_from_slice(&[0x55, 0xAA]);
            std::fs::write(temp.path().join("no-boot.img"), disk).unwrap();
            "[[card.drives.media]]\nimage='no-boot.img'\nwrite='readonly'\n"
        } else {
            "empty=true\n"
        };
        let path = temp.path().join("machine.toml");
        std::fs::write(
            &path,
            format!(
                "schema=1\nextends='preset:ibm-5170'\n[board.rtc]\nstart='2000-01-01T00:00:00'\nnvram='derived'\n[memory]\nextended_kib=0\n[firmware.bios]\nimage={}\nsha256='6938dc8b665fa120f33775163ab6bacc559389525db87adedf734416f9ef2442'\nprovides=['floppy']\n[[card]]\nid='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.2M'\n{first}[[card.drives.media]]\nimage={}\nsha256='4ee55e9c44d553041b9bbde99cacae31472f12debf6c9b9c09c5b9343c435d7e'\nwrite='readonly'\n",
                quoted(&bios),
                quoted(&floppy)
            ),
        )
        .unwrap();
        Some(path)
    }

    fn screen_text(shell: &mut Shell) -> String {
        let presenter = shell.presenter.as_mut().unwrap();
        let display = presenter.display;
        let capture = presenter.assembled().capture_text(display).unwrap();
        match &capture.frame {
            video::Frame::Text(grid) => grid.rows_as_strings().join("\n"),
            _ => String::new(),
        }
    }

    /// Tick in 100 ms host frames until the screen shows `needle`, within a
    /// bound of emulated time.
    fn tick_until_screen_shows(
        shell: &mut Shell,
        elapsed: &mut std::time::Duration,
        needle: &str,
        done: impl Fn(&Shell) -> bool,
    ) {
        let ctx = egui::Context::default();
        let bound = *elapsed + std::time::Duration::from_secs(120);
        while !(done(shell) && screen_text(shell).contains(needle)) {
            assert!(
                *elapsed < bound,
                "no {needle:?} on screen:\n{}",
                screen_text(shell)
            );
            *elapsed += std::time::Duration::from_millis(100);
            shell.tick(&ctx, false, *elapsed);
        }
    }

    #[test]
    #[ignore = "slow: open AT BIOS POST twice and a FreeDOS 1.2M boot through the shell"]
    fn no_rom_basic_takes_a_boot_floppy_and_ctrl_alt_del_boots_freedos() {
        let temp = machine_file::test_support::TempDir::new("shell-no-rom-basic");
        let Some(path) = open_bios_floppy_machine(&temp, true) else {
            return;
        };
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.presenter = None;
        shell.library = vec![LibraryEntry::inspect_with_rom_dirs(
            path,
            shell.args.options,
            &[],
        )];
        shell.selected = Some(0);
        shell.refresh_library();
        finish_readiness(&mut shell);
        shell.command(Command::Start, &ctx);
        finish_readiness(&mut shell);
        shell.presenter.as_mut().unwrap().full_slices = true;
        let mut elapsed = std::time::Duration::ZERO;
        // The boot code gives up: INT 18h prints its message and halts with
        // interrupts off, as clone AT BIOSes do while they wait for reset.
        tick_until_screen_shows(&mut shell, &mut elapsed, "No ROM BASIC", |shell| {
            shell.presenter.as_ref().unwrap().halted
        });
        assert!(shell.message.contains("press Ctrl+Alt+Del"));
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().select(Some(1));
        shell.confirm_insert();
        let resets = shell.presenter.as_ref().unwrap().machine().cpu_resets();
        shell.command(Command::Reset, &ctx);
        let presenter = shell.presenter.as_mut().unwrap();
        assert!(presenter.running);
        assert_eq!(presenter.machine().cpu_resets(), resets + 1);
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "freedos-1200.img"
        );
        tick_until_screen_shows(&mut shell, &mut elapsed, "FreeDOS kernel", |_| true);
        assert!(shell.presenter.as_ref().unwrap().error.is_none());
        assert!(shell.stop());
    }

    #[test]
    #[ignore = "slow: open AT BIOS POST and a FreeDOS 1.2M boot through the shell"]
    fn a_floppy_inserted_before_start_boots_freedos_on_the_open_bios() {
        let temp = machine_file::test_support::TempDir::new("shell-pending-boot");
        let Some(path) = open_bios_floppy_machine(&temp, false) else {
            return;
        };
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.presenter = None;
        shell.library = vec![LibraryEntry::inspect_with_rom_dirs(
            path,
            shell.args.options,
            &[],
        )];
        shell.selected = Some(0);
        shell.refresh_library();
        finish_readiness(&mut shell);
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().select(Some(0));
        shell.confirm_insert();
        shell.command(Command::Start, &ctx);
        finish_readiness(&mut shell);
        shell.presenter.as_mut().unwrap().full_slices = true;
        let mut elapsed = std::time::Duration::ZERO;
        tick_until_screen_shows(&mut shell, &mut elapsed, "FreeDOS kernel", |_| true);
        assert!(!shell.presenter.as_ref().unwrap().halted);
        assert!(shell.stop());
    }

    #[test]
    fn failed_finish_drops_session_and_exit_remains_available() {
        let temp = machine_file::test_support::TempDir::new("shell-failed-finish");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled().persistence[0].fail_output();
        assert!(!shell.stop());
        assert!(shell.presenter.is_none());
        assert!(shell.dialog.is_some());
        assert!(shell.errors.borrow().is_some());
        let ctx = egui::Context::default();
        let mut output = ctx.run_pass(Default::default(), |ctx| shell.command(Command::Exit, ctx));
        output.textures_delta.clear();
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close)));
        assert!(shell.errors.borrow().is_some());
        assert!(!output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::CancelClose)));
        output.drop_without_applying_deltas();
        assert_eq!(
            std::fs::metadata(temp.path().join("disk.upfj"))
                .unwrap()
                .len(),
            128
        );
    }

    #[test]
    fn title_about_and_menu_name_the_desktop_unemu() {
        let former = image_store::location::preferences::FORMER_DIRECTORY;
        let mut shell = shell();
        assert_eq!(shell.title(), "Machine — unemu");
        shell.presenter = None;
        assert_eq!(shell.title(), "unemu");
        assert_eq!(crate::app_menu::APP_NAME, "unemu");
        let menu = format!("{:?}", crate::app_menu::application_menu());
        assert!(menu.contains("\"About unemu\"") && menu.contains("\"Quit unemu\""));
        assert!(!menu.to_ascii_lowercase().contains(former), "{menu}");
        let ctx = egui::Context::default();
        shell.command(Command::About, &ctx);
        let (title, text) = shell.dialog.as_ref().unwrap();
        assert_eq!(*title, "About unemu");
        assert!(
            text.starts_with(&format!("unemu {}\n", env!("CARGO_PKG_VERSION"))),
            "{text}"
        );
        assert!(!text.to_ascii_lowercase().contains(former), "{text}");
        // The help names the default machine file and its directory, whose
        // paths depend on the host's data directory, cargo or a developer's switch.
        let default = crate::shown_path(crate::default_machine());
        let directory = crate::shown_path(crate::machines_dir());
        for option in ["--help", "--version", "--about"] {
            let output = crate::informational_output(std::ffi::OsStr::new(option))
                .unwrap()
                .replace(&default, "machines/at-freedos.toml")
                .replace(&directory, "machines");
            assert!(!output.to_ascii_lowercase().contains(former), "{output}");
        }
        assert!(crate::informational_output(std::ffi::OsStr::new("--help"))
            .unwrap()
            .starts_with("usage: unemu [--machine FILE]"));
    }

    #[test]
    fn embedded_font_credits_reach_about_and_informational_output() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.command(Command::About, &ctx);
        let text = &shell.dialog.as_ref().unwrap().1;
        assert!(text.contains(unemu_pc_core::FONT_NOTICE));
        assert!(text.contains(ui_kit::EGUI_FONT_NOTICE));
        assert!(text.contains(ui_kit::SYMBOL_FONT_NOTICE));
        assert!(text.contains(devices::OPL3_NOTICE));
        assert!(text.contains(&platforms::licence_notice()));
        for option in ["--help", "--about", "--version"] {
            let output = crate::informational_output(std::ffi::OsStr::new(option)).unwrap();
            assert!(output.contains(ui_kit::EGUI_FONT_NOTICE));
            assert!(output.contains(ui_kit::SYMBOL_FONT_NOTICE));
            assert!(output.contains(devices::OPL3_NOTICE));
        }
        for credit in [
            "Ubuntu-font-1.0",
            "OFL-1.1",
            "Source Foundry",
            "Canonical",
            "John Slegers",
            "Bitstream Vera Fonts Copyright",
            "Tavmjong Bah",
        ] {
            assert!(text.contains(credit));
        }
    }

    #[test]
    fn keyboard_zoom_is_disabled_for_the_shell() {
        let ctx = egui::Context::default();
        configure_context(&ctx);
        assert!(!ctx.options(|options| options.zoom_with_keyboard));
    }

    #[test]
    fn occlusion_is_processed_without_a_visible_ui_pass() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.key(KeyCode::KeyA, Pressed, false, &ctx);
        shell
            .events
            .borrow_mut()
            .window(WindowEvent::Occluded(true));
        shell.events.borrow_mut().key(KeyCode::KeyA, Released);
        shell.visibility(&ctx, false);
        assert!(shell.hidden);
        assert_eq!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .keyboard()
                .pending_events(),
            2
        );
        assert!(shell.modal_paused);
        assert!(shell.presenter.as_ref().unwrap().occluded_since.is_some());
        shell
            .events
            .borrow_mut()
            .window(WindowEvent::Occluded(false));
        shell.visibility(&ctx, false);
        assert!(!shell.hidden);
        assert!(!shell.modal_paused);
        assert!(shell.presenter.as_ref().unwrap().occluded_since.is_none());
    }
    #[test]
    fn exit_closes_even_when_it_discovers_a_finish_error() {
        let temp = machine_file::test_support::TempDir::new("shell-exit-finish-error");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled().persistence[0].fail_output();
        let ctx = egui::Context::default();
        let mut output = ctx.run_pass(Default::default(), |ctx| {
            shell.command(Command::Exit, ctx);
            assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
            shell.answer_stop_prompt(true, ctx);
        });
        output.textures_delta.clear();
        assert!(shell.presenter.is_none());
        assert!(shell.errors.borrow().is_some());
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Close)));
        output.drop_without_applying_deltas();
    }
    #[test]
    fn clean_start_and_stop_clear_an_old_error() {
        let mut shell = shell();
        shell.show_error("previous failure".into());
        let presenter = self::shell().presenter.take().unwrap();
        shell.started(presenter);
        assert!(shell.errors.borrow().is_none());
        assert!(shell.dialog.is_none());
        *shell.errors.borrow_mut() = Some("previous failure".into());
        assert!(shell.stop());
        assert!(shell.errors.borrow().is_none());
    }

    fn discovery_shell(temp: &machine_file::test_support::TempDir) -> Shell {
        use sha2::{Digest, Sha256};
        let root = temp.path().join("roms");
        std::fs::create_dir(&root).unwrap();
        let bytes = vec![0xf4; 32768];
        std::fs::write(root.join("bios"), &bytes).unwrap();
        let path = temp.path().join("machine.toml");
        std::fs::write(
            &path,
            format!(
                "schema=1
[board]
model='ibm-5170'
[firmware.bios]
image='absent-bios'
sha256='{:x}'
[[card]]
kind='mda'
",
                Sha256::digest(&bytes)
            ),
        )
        .unwrap();
        let mut shell = shell();
        shell.presenter = None;
        shell.args.options.allow_unlisted_firmware = true;
        shell.library = vec![LibraryEntry::pending(path)];
        shell.selected = Some(0);
        shell.preferences.add_folder(&root).unwrap();
        shell.preferences_draft = shell.preferences.clone();
        shell.firmware_cache = Some(temp.path().join("cache/index"));
        shell
    }

    fn finish_discovery(
        shell: &mut Shell,
        ctx: &egui::Context,
    ) -> machine_file::firmware_discovery::DiscoveryProgress {
        shell.poll_readiness(ctx);
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        let progress = job.control.snapshot();
        shell.apply_readiness(job.index, job.revision, prepared);
        progress
    }

    #[test]
    fn readiness_uses_preferences_store_then_injected_default_without_rom_folders() {
        let temp = machine_file::test_support::TempDir::new("readiness-store-root");
        let root = temp.path().join("configured");
        let store = image_store::Store::open(&root).unwrap();
        let bytes = vec![0xf4; 32768];
        let path = temp.path().join("source.bin");
        std::fs::write(&path, &bytes).unwrap();
        let digest = image_store::Digest::of(&bytes);
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc'\nname='Synthetic readiness'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n",digest.hex(),digest).as_bytes()).unwrap();
        let candidate = pc_image::firmware_import::Candidate {
            catalogue_id: "readiness".into(),
            config,
            sources: vec![path],
            distribution: assembly::FirmwareDistribution::UserSupplied,
        };
        let package = pc_image::firmware_import::publish(
            &store,
            &candidate,
            &[],
            pc_image::firmware::catalogue(),
        )
        .unwrap();
        let machine = temp.path().join("machine.toml");
        std::fs::write(&machine,format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='absent.bin'\nsha256='{}'\n[[card]]\nkind='mda'\n",digest.hex())).unwrap();
        for preference in [true, false] {
            let mut shell = shell();
            shell.presenter = None;
            shell.library = vec![LibraryEntry::pending(machine.clone())];
            shell.selected = Some(0);
            shell.args.options.allow_unlisted_firmware = true;
            if preference {
                shell.preferences.store_root = Some(root.clone());
                shell.default_store_root = Some(temp.path().join("wrong-default"));
            } else {
                shell.default_store_root = Some(root.clone());
            }
            let progress = finish_discovery(&mut shell, &egui::Context::default());
            assert!(
                !shell.library[0].has_problem(),
                "{:?}",
                shell.library[0].warning
            );
            assert_eq!(progress.folder_listings, 0);
            assert_eq!(
                pc_image::firmware::firmware_sources(&shell.prepared[&0]).unwrap(),
                std::slice::from_ref(&package.digest)
            );
        }
    }

    #[test]
    fn start_status_remains_loading_while_discovery_progress_is_pending() {
        let mut shell = shell();
        shell.presenter = None;
        shell.library = vec![LibraryEntry::pending("synthetic.toml".into())];
        shell.selected = Some(0);
        shell.start();
        let (_sender, receiver) = std::sync::mpsc::channel();
        shell.readiness = Some(ReadinessJob {
            index: 0,
            revision: 0,
            user_cancelled: false,
            control: shell.discovery_control.clone(),
            receiver,
        });
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(1600.0);
            shell.status(ui, &mut Vec::new());
        });
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text().contains("Loading machine…"))
        }));
        output.drop_without_applying_deltas();
    }

    #[test]
    fn the_library_sidebar_has_no_logo_entry() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.logo = Some(ui_kit::logo::Logo::load(&ctx).unwrap());
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 700.0),
                )),
                ..Default::default()
            },
            |ui| shell.welcome(ui, &mut Vec::new()),
        );
        let texts: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        for entry in [
            "Library",
            "Machines (0)",
            "Images (0)",
            "Installation media (0)",
            "Firmware (0)",
            "Machine files (0)",
            "Remotes",
        ] {
            assert!(texts.iter().any(|t| t == entry), "{entry}: {texts:?}");
        }
        assert!(!texts.iter().any(|t| t == "unemu"), "{texts:?}");
    }

    #[test]
    fn checked_start_has_zero_discovery_reads_without_an_error_hint() {
        let temp = machine_file::test_support::TempDir::new("checked-start-reuse");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        let checked = finish_discovery(&mut shell, &ctx);
        assert_eq!(checked.folder_listings, 1);
        assert_eq!(checked.files_read, 1);
        assert!(!shell.library[0].has_problem());
        shell.start();
        assert_eq!(shell.message, "Loading machine…");
        assert_eq!(
            shell.library[0].warning.as_deref(),
            Some("Loading machine…")
        );
        assert!(!shell.library[0].has_problem());
        assert!(!shell.library[0].hint().contains("missing"));
        assert!(shell.errors.borrow().is_none());
        let launched = finish_discovery(&mut shell, &ctx);
        assert_eq!(launched.folder_listings, 0);
        assert_eq!(launched.files_read, 0);
        assert_eq!(launched.checked, 0);
        assert!(shell.presenter.is_some());
        assert!(shell.errors.borrow().is_none());
    }

    #[test]
    fn changed_located_rom_relists_only_its_folder_and_retries_once() {
        let temp = machine_file::test_support::TempDir::new("changed-start-rom");
        let mut shell = discovery_shell(&temp);
        let untouched = temp.path().join("untouched");
        std::fs::create_dir(&untouched).unwrap();
        std::fs::write(untouched.join("other"), [0; 512]).unwrap();
        shell.preferences.add_folder(&untouched).unwrap();
        let ctx = egui::Context::default();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 2);
        std::fs::rename(
            temp.path().join("roms/bios"),
            temp.path().join("roms/replacement"),
        )
        .unwrap();
        std::fs::write(temp.path().join("roms/bios"), [0; 32768]).unwrap();
        shell.start();
        let retried = finish_discovery(&mut shell, &ctx);
        assert_eq!(retried.folder_listings, 1);
        assert_eq!(retried.files_read, 3);
        assert!(shell.presenter.is_some());
        assert!(shell.errors.borrow().is_none());
    }

    #[test]
    fn missing_replacement_reports_only_the_final_failure_without_looping() {
        let temp = machine_file::test_support::TempDir::new("failed-start-retry");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        finish_discovery(&mut shell, &ctx);
        std::fs::write(temp.path().join("roms/bios"), [0; 32768]).unwrap();
        shell.start();
        assert!(shell.errors.borrow().is_none());
        let failed = finish_discovery(&mut shell, &ctx);
        assert_eq!(failed.folder_listings, 1);
        assert_eq!(failed.files_read, 1);
        assert!(shell.presenter.is_none());
        assert!(shell.errors.borrow().is_some());
        shell.poll_readiness(&ctx);
        assert!(shell.readiness.is_none());
    }

    #[test]
    fn folder_add_scans_once_and_selection_reuses_the_session_listing() {
        let temp = machine_file::test_support::TempDir::new("folder-add-scan");
        let mut shell = discovery_shell(&temp);
        shell.preferences = Default::default();
        shell.preferences_draft = Default::default();
        let second = temp.path().join("second.toml");
        std::fs::copy(&shell.library[0].path, &second).unwrap();
        shell.library.push(LibraryEntry::pending(second));
        let ctx = egui::Context::default();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 0);
        shell.dialog = Some(("ROM folders", String::new()));
        shell
            .preferences_draft
            .add_folder(&temp.path().join("roms"))
            .unwrap();
        shell.poll_readiness(&ctx);
        assert!(shell.readiness.is_none());
        shell.preferences_path = Some(temp.path().join("preferences.toml"));
        shell.save_preferences();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 1);
        assert!(!shell.library[0].has_problem());
        shell.selected = Some(1);
        let selected = finish_discovery(&mut shell, &ctx);
        assert_eq!(selected.folder_listings, 0);
        assert_eq!(selected.files_read, 1);
        shell.selected = Some(0);
        shell.poll_readiness(&ctx);
        assert!(shell.readiness.is_none());
    }

    #[test]
    fn automatic_check_recovers_a_rom_moved_into_a_subfolder() {
        let temp = machine_file::test_support::TempDir::new("moved-session-rom");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        finish_discovery(&mut shell, &ctx);
        shell.start();
        finish_discovery(&mut shell, &ctx);
        assert!(shell.stop());
        let nested = temp.path().join("roms/nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::rename(temp.path().join("roms/bios"), nested.join("bios")).unwrap();
        let recovered = finish_discovery(&mut shell, &ctx);
        assert_eq!(recovered.folder_listings, 2);
        assert_eq!(recovered.files_read, 1);
        assert!(!shell.library[0].has_problem());
        shell.check_firmware_again();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 2);
    }

    #[test]
    fn saved_folder_changes_keep_existing_listings_and_drafts_do_not_scan() {
        let temp = machine_file::test_support::TempDir::new("incremental-rom-folders");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        finish_discovery(&mut shell, &ctx);
        shell.preferences_path = Some(temp.path().join("preferences.toml"));
        let added = temp.path().join("added");
        std::fs::create_dir(&added).unwrap();
        shell.dialog = Some(("ROM folders", String::new()));
        shell.preferences_draft.add_folder(&added).unwrap();
        shell.poll_readiness(&ctx);
        assert!(shell.readiness.is_none());
        shell.save_preferences();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 1);
        shell.preferences_draft.rom_dirs.pop();
        shell.save_preferences();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 0);
        let revision = shell.library_revision;
        shell.dialog = Some(("ROM folders", String::new()));
        shell.preferences_draft.rom_dirs.clear();
        shell.poll_readiness(&ctx);
        shell.close_firmware_dialog();
        shell.poll_readiness(&ctx);
        assert_eq!(shell.library_revision, revision);
        assert!(shell.readiness.is_none());
    }

    #[test]
    fn saved_folder_changes_defer_readiness_while_a_machine_runs() {
        let temp = machine_file::test_support::TempDir::new("running-rom-folders");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        finish_discovery(&mut shell, &ctx);
        shell.presenter = self::shell().presenter.take();
        shell.preferences_path = Some(temp.path().join("preferences.toml"));
        let added = temp.path().join("added");
        std::fs::create_dir(&added).unwrap();
        shell.preferences_draft.add_folder(&added).unwrap();
        let revision = shell.library_revision;
        shell.save_preferences();
        shell.poll_readiness(&ctx);
        assert_eq!(shell.library_revision, revision);
        assert!(shell.library[0].checked);
        assert!(shell.readiness.is_none());
        assert!(shell.stop());
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 1);
    }

    #[test]
    fn abandoned_start_resets_loading_after_selection_cancel_and_disconnect() {
        for action in 0..3 {
            let temp = machine_file::test_support::TempDir::new("abandoned-start-status");
            let mut shell = discovery_shell(&temp);
            shell.discovery_roots = shell
                .preferences
                .roots(&shell.args.rom_dirs, &shell.default_firmware_roots);
            shell.checked_selection = Some(0);
            let mut other = LibraryEntry::pending(temp.path().join("other.toml"));
            other.checked = true;
            other.warning = None;
            shell.library.push(other);
            shell.start();
            let (sender, receiver) = std::sync::mpsc::channel();
            shell.readiness = Some(ReadinessJob {
                index: 0,
                revision: shell.library_revision,
                user_cancelled: false,
                control: shell.discovery_control.clone(),
                receiver,
            });
            match action {
                0 => shell.selected = Some(1),
                1 => shell.cancel_firmware(),
                _ => drop(sender),
            }
            shell.poll_readiness(&egui::Context::default());
            assert_eq!(shell.message, "Ready");
            assert!(shell.start_request.is_none());
        }
    }

    #[test]
    fn changed_machine_document_is_not_replaced_by_the_checked_document() {
        let temp = machine_file::test_support::TempDir::new("changed-machine-start");
        let mut shell = discovery_shell(&temp);
        let path = shell.library[0].path.clone();
        let parent = temp.path().join("parent.toml");
        std::fs::rename(&path, &parent).unwrap();
        std::fs::write(&path, "schema=1\nextends='parent.toml'\n").unwrap();
        let ctx = egui::Context::default();
        finish_discovery(&mut shell, &ctx);
        let previous_memory = shell.library[0]
            .details
            .iter()
            .find(|(name, _)| name == "Memory")
            .cloned();
        let contents = std::fs::read_to_string(&parent).unwrap();
        std::fs::write(
            &parent,
            format!("{contents}\n[memory]\nconventional_kib=512\nextended_kib=0\n"),
        )
        .unwrap();
        shell.start();
        assert_eq!(finish_discovery(&mut shell, &ctx).folder_listings, 0);
        assert_ne!(
            shell.library[0]
                .details
                .iter()
                .find(|(name, _)| name == "Memory")
                .cloned(),
            previous_memory
        );
        assert_eq!(
            shell.prepared[&0]
                .resolved()
                .0
                .memory
                .as_ref()
                .unwrap()
                .conventional_kib,
            Some(512)
        );
        assert!(shell.presenter.is_some());
    }

    #[test]
    fn discovery_job_uses_only_explicit_and_injected_roots() {
        use machine_file::firmware_discovery::{FirmwareRoot, RootOrigin};
        let temp = machine_file::test_support::TempDir::new("isolated-shell-discovery");
        let mut shell = discovery_shell(&temp);
        let env_like = FirmwareRoot::new(
            temp.path().join("missing-env-like"),
            RootOrigin::Environment,
        );
        shell.default_firmware_roots = vec![env_like.clone()];
        assert_eq!(
            shell
                .preferences
                .roots(&shell.args.rom_dirs, &shell.default_firmware_roots),
            vec![
                FirmwareRoot::new(
                    temp.path().join("roms").canonicalize().unwrap(),
                    RootOrigin::Preferences
                ),
                env_like
            ],
        );
        shell.poll_readiness(&egui::Context::default());
        let job = shell.readiness.take().unwrap();
        let (entry, prepared) = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        assert!(prepared.is_some());
        assert_eq!(job.control.snapshot().listed, 1);
        assert_eq!(job.control.snapshot().files_read, 1);
        assert_eq!(job.control.snapshot().matches, 1);
        assert_eq!(entry.discovery_warnings.len(), 1);
        assert!(entry.discovery_warnings[0].starts_with("<UNEMUPC_USER_ROMS_DIR>: skipped"));
    }

    #[test]
    fn cancel_and_rescan_preserve_dialog_disable_start_and_discard_old_readiness() {
        let temp = machine_file::test_support::TempDir::new("cancel-rescan-readiness");
        let mut shell = discovery_shell(&temp);
        shell.dialog = Some(("ROM folders", String::new()));
        let ctx = egui::Context::default();
        shell.poll_readiness(&ctx);
        assert!(!shell.enablement().allows(Command::Start));
        let mut job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        assert!(job.control.snapshot().listed > 0);
        assert!(job.control.snapshot().files_read > 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(prepared).unwrap();
        job.receiver = receiver;
        shell.readiness = Some(job);
        shell.cancel_firmware();
        shell.poll_readiness(&ctx);
        assert!(shell.readiness.is_none());
        assert!(shell.library[0].checked);
        assert!(shell.library[0]
            .warning
            .as_ref()
            .unwrap()
            .contains("cancelled"));
        assert!(shell.library[0].hint().contains("Check again"));
        assert!(!shell.library[0].hint().contains("Rescan"));
        assert!(!shell.enablement().allows(Command::Start));
        shell.rescan_firmware();
        assert!(shell.rescan);
        assert!(shell.dialog.is_some());
        shell.poll_readiness(&ctx);
        assert!(!shell.rescan);
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        assert!(!job.control.is_cancelled());
        shell.apply_readiness(job.index, job.revision, prepared);
        assert!(shell.enablement().allows(Command::Start));
    }

    #[test]
    fn closing_folders_discards_drafts_without_touching_a_saved_job() {
        let temp = machine_file::test_support::TempDir::new("close-firmware-folders");
        let mut shell = discovery_shell(&temp);
        let ctx = egui::Context::default();
        shell.poll_readiness(&ctx);
        let control = shell.readiness.as_ref().unwrap().control.clone();
        let revision = shell.library_revision;
        shell.dialog = Some(("ROM folders", String::new()));
        shell.preferences_draft.rom_dirs.clear();
        shell.poll_readiness(&ctx);
        shell.close_firmware_dialog();
        shell.poll_readiness(&ctx);
        assert!(!control.is_cancelled());
        assert_eq!(shell.library_revision, revision);
        if let Some(job) = shell.readiness.take() {
            job.receiver
                .recv_timeout(crate::test_wait::HANG_GUARD)
                .unwrap();
        }
    }

    #[test]
    fn superseded_selection_is_not_a_user_cancel_even_when_selected_again() {
        let temp = machine_file::test_support::TempDir::new("superseded-firmware-check");
        let mut shell = discovery_shell(&temp);
        shell.checked_selection = Some(0);
        shell
            .library
            .push(LibraryEntry::pending(temp.path().join("second.toml")));
        let (sender, receiver) = std::sync::mpsc::channel();
        shell.readiness = Some(ReadinessJob {
            index: 0,
            revision: shell.library_revision,
            user_cancelled: false,
            control: Default::default(),
            receiver,
        });
        shell.selected = Some(1);
        shell.start_request = Some(0);
        shell.message = "Rescanning ROM folders…".into();
        shell.poll_readiness(&egui::Context::default());
        assert!(shell.start_request.is_none());
        assert_eq!(shell.message, "Ready");
        assert!(!shell.discovery_finished);
        assert!(!shell.visible_discovery());
        shell.selected = Some(0);
        sender
            .send((LibraryEntry::pending(shell.library[0].path.clone()), None))
            .unwrap();
        shell.poll_readiness(&egui::Context::default());
        assert!(!shell.library[0].checked);
        assert!(!shell.library[0]
            .warning
            .as_ref()
            .unwrap()
            .contains("cancelled"));
        let job = shell.readiness.take().unwrap();
        assert!(!job.user_cancelled);
        job.receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
    }

    #[test]
    fn discovery_uses_the_existing_status_cell_without_changing_height() {
        use machine_file::firmware_discovery::{DiscoveryPhase, DiscoveryProgress};
        let mut shell = shell();
        assert_eq!(
            shell.discovery_control.snapshot().phase,
            DiscoveryPhase::Idle
        );
        let ctx = egui::Context::default();
        let mut heights = Vec::new();
        for active in [false, true] {
            if active {
                let (_sender, receiver) = std::sync::mpsc::channel();
                shell.discovery_control.loading();
                shell.readiness = Some(ReadinessJob {
                    index: 0,
                    revision: 0,
                    user_cancelled: false,
                    control: shell.discovery_control.clone(),
                    receiver,
                });
            }
            let mut output = ctx.run_ui(Default::default(), |ui| {
                shell.status(ui, &mut Vec::new());
                heights.push(ui.min_rect().height());
            });
            let single_line = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        Some(text)
                    } else {
                        None
                    }
                })
                .all(|text| text.galley.rows.len() <= 1);
            output.textures_delta.clear();
            output.drop_without_applying_deltas();
            assert!(single_line);
        }
        assert_eq!(heights[0], heights[1]);
        let mut output = ctx.run_ui(Default::default(), |ui| {
            let before = ui.min_rect();
            firmware_progress(
                ui,
                &DiscoveryProgress {
                    phase: DiscoveryPhase::Idle,
                    ..Default::default()
                },
                false,
            );
            assert_eq!(before, ui.min_rect());
        });
        output.textures_delta.clear();
        output.drop_without_applying_deltas();
    }

    #[test]
    fn cancel_discards_a_queued_rescan_before_the_previous_worker_finishes() {
        let temp = machine_file::test_support::TempDir::new("cancel-queued-rescan");
        let mut shell = shell();
        shell.presenter = None;
        let path = temp.path().join("machine.toml");
        shell.library = vec![LibraryEntry::pending(path.clone())];
        shell.selected = Some(0);
        shell.checked_selection = Some(0);
        let (sender, receiver) = std::sync::mpsc::channel();
        shell.readiness = Some(ReadinessJob {
            index: 0,
            revision: shell.library_revision,
            user_cancelled: false,
            control: Default::default(),
            receiver,
        });
        shell.rescan_firmware();
        shell.cancel_firmware();
        assert!(!shell.rescan);
        sender.send((LibraryEntry::pending(path), None)).unwrap();
        shell.poll_readiness(&egui::Context::default());
        assert!(shell.readiness.is_none());
        assert!(!shell.enablement().allows(Command::Start));
    }

    #[test]
    fn firmware_progress_renders_headlessly_in_each_phase() {
        use machine_file::firmware_discovery::{DiscoveryPhase, DiscoveryProgress};
        let ctx = egui::Context::default();
        for phase in [
            DiscoveryPhase::Listing,
            DiscoveryPhase::Hashing,
            DiscoveryPhase::Loading,
            DiscoveryPhase::Cancelled,
            DiscoveryPhase::Complete,
        ] {
            let progress = DiscoveryProgress {
                phase,
                listed: 20,
                candidates: 4,
                checked: 2,
                matches: 1,
                ..Default::default()
            };
            let mut output = ctx.run_ui(Default::default(), |ui| {
                firmware_progress(ui, &progress, true);
            });
            let expected = match phase {
                DiscoveryPhase::Loading => "Loading machine…",
                DiscoveryPhase::Complete => "Firmware check complete",
                DiscoveryPhase::Cancelled => "Firmware check cancelled",
                _ => "Scanning ROM folders…",
            };
            let has_progress = output.shapes.iter().any(|shape| {
                matches!(&shape.shape, egui::epaint::Shape::Text(text) if text.galley.text().contains(expected) && text.galley.rows.len() <= 1)
            });
            output.textures_delta.clear();
            output.drop_without_applying_deltas();
            assert!(has_progress);
        }
    }

    #[test]
    fn readiness_checks_only_selection_and_ignores_stale_results() {
        let temp = machine_file::test_support::TempDir::new("selected-firmware-readiness");
        let mut shell = shell();
        shell.presenter = None;
        shell.library = vec![
            LibraryEntry::pending(temp.path().join("first.toml")),
            LibraryEntry::pending(temp.path().join("second.toml")),
        ];
        shell.selected = Some(0);
        let ctx = egui::Context::default();
        shell.poll_readiness(&ctx);
        let job = shell.readiness.take().unwrap();
        assert_eq!(job.index, 0);
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        shell.refresh_library();
        shell.apply_readiness(job.index, job.revision, prepared);
        assert!(!shell.library[0].checked);
        assert!(!shell.library[1].checked);
        shell.poll_readiness(&ctx);
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        shell.apply_readiness(job.index, job.revision, prepared);
        assert!(shell.library[0].checked);
        assert!(!shell.library[1].checked);
        shell.start();
        assert_eq!(shell.start_request, Some(0));
        assert!(!shell.library[0].checked);
        assert!(shell.presenter.is_none());
    }

    #[test]
    fn saved_rom_folders_refresh_readiness_and_feed_launch_loading() {
        use sha2::{Digest, Sha256};
        let temp = machine_file::test_support::TempDir::new("shell-rom-readiness");
        let dumps = temp.path().join("dumps");
        std::fs::create_dir(&dumps).unwrap();
        let bytes = vec![0xf4; 32768];
        std::fs::write(dumps.join("renamed.bin"), &bytes).unwrap();
        let path = temp.path().join("machine.toml");
        std::fs::write(&path, format!("schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='absent.bin'\nsha256='{:x}'\n[[card]]\nkind='mda'\n", Sha256::digest(&bytes))).unwrap();
        let mut shell = shell();
        shell.presenter = None;
        shell.args.machine = path.clone();
        shell.args.options.allow_unlisted_firmware = true;
        shell.library = vec![LibraryEntry::inspect_with_rom_dirs(
            path,
            shell.args.options,
            &[],
        )];
        shell.selected = Some(0);
        assert!(!shell.enablement().allows(Command::Start));
        shell.preferences_path = Some(temp.path().join("config/preferences.toml"));
        shell.preferences_draft.add_folder(&dumps).unwrap();
        shell.save_preferences();
        assert!(!shell.enablement().allows(Command::Start));
        let ctx = egui::Context::default();
        shell.poll_readiness(&ctx);
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(crate::test_wait::HANG_GUARD)
            .unwrap();
        let document = prepared.1.as_ref().unwrap();
        let loaded = document.load(shell.args.options).unwrap();
        crate::setup_loaded(&shell.args, &loaded, &mut io::sink()).unwrap();
        shell.apply_readiness(job.index, job.revision, prepared);
        assert!(shell.enablement().allows(Command::Start));
        assert_eq!(
            crate::preferences::Preferences::load(shell.preferences_path.as_ref().unwrap())
                .unwrap(),
            shell.preferences
        );
        assert!(!temp.path().join("absent.bin").exists());
    }
    #[test]
    fn stopped_layer_review_cannot_preserve_a_later_disposable_session() {
        use crate::library_test_support::Fixture;
        let f = Fixture::new();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.stop();
        shell.args.options.allow_unlisted_firmware = true;
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell.image_library.selected = Some(f.row.clone());
        shell.poll_images(&ctx);
        shell.command(Command::GamingMode, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.presenter.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
        let name = shell.image_library.active.as_ref().unwrap().1.clone();
        let action = crate::library_view::Request::Discard("earlier-workspace".into());
        assert!(!shell.stop_for_image_action(&action));
        assert!(shell.presenter.is_none());
        assert!(shell.image_library.needs_decision());
        shell.image_library.shutdown();
        assert!(f
            .store
            .workspace_path(&name)
            .unwrap()
            .join("desktop.toml")
            .exists());
    }
    #[test]
    fn settings_on_an_image_and_save_as_new_image_open_from_the_machine_menu() {
        use crate::library_test_support::Fixture;
        let f = Fixture::new();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.stop();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        assert!(!shell.command_allowed(Command::SaveImage));
        shell.image_library.selected = Some(f.row.clone());
        assert!(shell.command_allowed(Command::Settings));
        shell.command(Command::Settings, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.image_library.image_settings.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let settings = shell.image_library.image_settings.as_ref().unwrap();
        assert_eq!(settings.row.reference, "test:source");
        assert!(!settings.read_only);
        assert!(shell.has_dialog());
        assert!(shell.settings.is_none(), "not the machine-file sheet");
        shell.image_library.image_settings = None;
        assert!(shell.command_allowed(Command::SaveImage));
        shell.command(Command::SaveImage, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell
                .image_library
                .layers
                .as_ref()
                .is_some_and(|dialog| dialog.save.is_some())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let form = shell
            .image_library
            .layers
            .as_ref()
            .and_then(|dialog| dialog.save.as_ref())
            .unwrap();
        assert_eq!(form.source, "test:source");
        assert!(shell.has_dialog());
        shell.image_library.shutdown();
    }
    #[test]
    fn unchecked_image_start_command_checks_then_launches_and_blockers_disable_start() {
        use crate::library_test_support::Fixture;
        let f = Fixture::new();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.stop();
        shell.args.options.allow_unlisted_firmware = true;
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell.image_library.selected = Some(f.row.clone());
        assert!(shell.image_library.readiness.is_none());
        assert!(shell.command_allowed(Command::Start));
        shell.command(Command::Start, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.presenter.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
        shell.stop();
        shell.image_library.shutdown();
        shell.image_library.readiness = Some(Err("Missing disk blob".into()));
        shell.image_library.busy = false;
        assert!(!shell.command_allowed(Command::Start));
    }
    #[test]
    fn media_import_waits_for_a_stopped_machine_and_sets_are_never_started() {
        let (f, cache) = crate::library_view::install_media::tests::library();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        assert!(shell.presenter.is_some());
        assert!(!shell.command_allowed(Command::ImportMedia));
        shell.stop();
        assert!(shell.command_allowed(Command::ImportMedia));
        shell.command(Command::ImportMedia, &ctx);
        assert!(shell.image_library.has_dialog());
        assert!(
            !shell.command_allowed(Command::ImportMedia),
            "one import at a time"
        );
        shell.image_library.close_dialogs();
        shell.image_library.selected = Some(f.row.clone());
        assert!(shell.command_allowed(Command::Start));
        shell
            .image_library
            .select_set(&cache, "example-adventure:r1", &mut shell.selected);
        assert!(!shell.command_allowed(Command::Start));
        assert!(!shell.command_allowed(Command::GamingMode));
        assert!(
            !shell.command_allowed(Command::InsertSet(0)),
            "no machine runs"
        );
        shell.image_library.shutdown();
    }
    #[test]
    fn firmware_import_waits_for_a_stopped_machine_and_packages_are_never_started() {
        use crate::library_test_support::Fixture;
        use crate::library_view::firmware::tests::{publish_open_package, warm};
        let f = Fixture::new();
        publish_open_package(&f);
        let cache = warm(&f.store);
        let mut shell = shell();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        assert!(shell.presenter.is_some());
        assert!(!shell.command_allowed(Command::ImportFirmware));
        shell.stop();
        assert!(shell.command_allowed(Command::ImportFirmware));
        shell.image_library.selected = Some(f.row.clone());
        assert!(shell.command_allowed(Command::Start));
        assert!(!shell.command_allowed(Command::EditFirmware), "no package");
        shell
            .image_library
            .select_first_package(&cache, &mut shell.selected);
        assert!(!shell.command_allowed(Command::Start));
        assert!(!shell.command_allowed(Command::GamingMode));
        assert!(shell.command_allowed(Command::EditFirmware));
        shell.image_library.shutdown();
    }

    /// Run one headless pass and return the viewport commands and painted text.
    fn pass(
        ctx: &egui::Context,
        input: egui::RawInput,
        run: impl FnMut(&egui::Context),
    ) -> (Vec<egui::ViewportCommand>, Vec<(String, egui::Rect)>) {
        let mut output = ctx.run_pass(input, run);
        output.textures_delta.clear();
        let commands = output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone();
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect();
        output.drop_without_applying_deltas();
        (commands, text)
    }

    fn shows(text: &[(String, egui::Rect)], wanted: &str) -> bool {
        text.iter().any(|(shown, _)| shown.contains(wanted))
    }

    fn guest_clock(shell: &Shell) -> u64 {
        shell
            .presenter
            .as_ref()
            .unwrap()
            .assembled_ref()
            .machine
            .clock()
            .0
    }

    #[test]
    fn running_machine_actions_ask_before_stopping_and_cancel_resumes() {
        use std::time::Duration;
        let picked = std::path::Path::new("picked.toml");
        for prompt in [
            StopPrompt::Library,
            StopPrompt::OpenMachine,
            StopPrompt::Exit,
        ] {
            let command = match prompt {
                StopPrompt::Library => Command::Library,
                StopPrompt::OpenMachine => Command::Placeholder("Open machine file"),
                StopPrompt::Exit => Command::Exit,
                StopPrompt::OpenFile => unreachable!("a file's prompt has its own test"),
            };
            // Open machine file asks only once a path is confirmed.
            let trigger = |shell: &mut Shell, ctx: &egui::Context| {
                shell.command(command, ctx);
                if prompt == StopPrompt::OpenMachine {
                    assert_eq!(shell.stop_prompt, None);
                    assert_eq!(
                        shell.dialog.as_ref().map(|(title, _)| *title),
                        Some("Open machine file")
                    );
                    shell.open_path = picked.to_string_lossy().into_owned();
                    shell.open_machine();
                }
            };
            for user_paused in [false, true] {
                let mut shell = shell();
                shell.paused = user_paused;
                let ctx = egui::Context::default();
                shell.presenter.as_mut().unwrap().full_slices = true;
                shell.presenter.as_mut().unwrap().next_paint = Some(Duration::ZERO);
                shell.tick(&ctx, false, Duration::from_millis(20));
                let before = guest_clock(&shell);
                assert_eq!(before == 0, user_paused, "{command:?}");

                let (commands, _) = pass(&ctx, Default::default(), |ctx| trigger(&mut shell, ctx));
                assert_eq!(shell.stop_prompt, Some(prompt));
                assert!(shell.presenter.as_ref().unwrap().running);
                assert!(shell.presenter.as_ref().unwrap().occluded_since.is_some());
                assert!(shell.has_dialog());
                assert!(shell.dialog.is_none(), "{command:?}");
                assert!(!shell.exit_requested);
                assert!(!commands.contains(&egui::ViewportCommand::Close));
                assert_eq!(
                    commands.contains(&egui::ViewportCommand::CancelClose),
                    prompt == StopPrompt::Exit
                );
                // No guest time passes while the question is open.
                shell.tick(&ctx, false, Duration::from_millis(200));
                assert_eq!(guest_clock(&shell), before);

                // Cancel resumes exactly where the machine paused, without
                // catching up the time the question was open.
                shell.answer_stop_prompt(false, &ctx);
                assert_eq!(shell.stop_prompt, None);
                assert!(!shell.has_dialog());
                assert_eq!(shell.paused, user_paused);
                assert!(shell.library.iter().all(|entry| entry.path != picked));
                shell.tick(&ctx, false, Duration::from_millis(400));
                shell.tick(&ctx, false, Duration::from_millis(440));
                let after = guest_clock(&shell);
                if user_paused {
                    assert_eq!(after, before);
                } else {
                    assert!(after > before);
                    assert!(
                        u128::from(after - before)
                            <= crate::pacing::virtual_ticks(Duration::from_millis(45))
                    );
                }

                // Stop machine stops the session and continues the action.
                let (commands, _) = pass(&ctx, Default::default(), |ctx| {
                    trigger(&mut shell, ctx);
                    shell.answer_stop_prompt(true, ctx);
                });
                assert!(shell.presenter.is_none());
                assert_eq!(shell.stop_prompt, None);
                match prompt {
                    StopPrompt::Library => assert!(shell.dialog.is_none()),
                    StopPrompt::OpenMachine => {
                        assert!(shell.dialog.is_none());
                        let selected = shell
                            .selected
                            .map(|index| shell.library[index].path.clone());
                        assert_eq!(selected.as_deref(), Some(picked));
                    }
                    StopPrompt::Exit => {
                        assert!(commands.contains(&egui::ViewportCommand::Close))
                    }
                    StopPrompt::OpenFile => unreachable!(),
                }
            }
        }
    }

    #[test]
    fn stopped_halted_and_failed_sessions_end_without_a_stop_prompt() {
        for case in ["stopped", "halted", "failed"] {
            for command in [Command::Library, Command::Exit] {
                let mut shell = shell();
                match case {
                    "stopped" => assert!(shell.stop()),
                    "halted" => {
                        let presenter = shell.presenter.as_mut().unwrap();
                        presenter.running = false;
                        presenter.halted = true;
                    }
                    _ => shell.presenter.as_mut().unwrap().error = Some("cpu failure".into()),
                }
                let ctx = egui::Context::default();
                let (commands, _) =
                    pass(&ctx, Default::default(), |ctx| shell.command(command, ctx));
                assert_eq!(shell.stop_prompt, None, "{case} {command:?}");
                assert!(shell.presenter.is_none(), "{case} {command:?}");
                assert_eq!(
                    commands.contains(&egui::ViewportCommand::Close),
                    command == Command::Exit
                );
            }
            let mut shell = shell();
            if case == "stopped" {
                shell.stop();
            }
            shell.presenter.iter_mut().for_each(|p| {
                p.running = case != "halted";
                p.halted = case == "halted";
            });
            if case == "failed" {
                shell.presenter.as_mut().unwrap().error = Some("cpu failure".into());
            }
            shell.command(
                Command::Placeholder("Open machine file"),
                &egui::Context::default(),
            );
            assert_eq!(shell.stop_prompt, None);
            assert_eq!(
                shell.dialog.as_ref().map(|(title, _)| *title),
                Some("Open machine file")
            );
        }
    }

    /// A shell whose machine file lives in `temp` and whose machine is the
    /// protected-mode guest the CPU core refuses.
    fn refusing_shell(temp: &machine_file::test_support::TempDir) -> Shell {
        let mut shell = shell();
        shell.args.machine = temp.path().join("refusal.toml");
        shell.presenter = Some(refusing_presenter());
        shell
    }

    /// A session of the protected-mode guest the CPU core refuses.
    fn refusing_presenter() -> Presenter {
        let config = assembly::test_support::refused_gate_call();
        let assembled = machine_file::assemble_with_observation_and_options(
            &config,
            &assembly::ObservationPolicy {
                routes: vec![assembly::ObservationRequest {
                    device: assembly::DeviceId::Vga,
                    output: assembly::OutputKind::TextScanout,
                    byte_limit: 0,
                }],
                ..assembly::ObservationPolicy::default()
            },
            assembly::AssemblyOptions {
                allow_unlisted_firmware: true,
            },
        )
        .unwrap();
        Presenter::from_assembled(&config, assembled, assembly::DeviceId::Vga, &mut io::sink())
            .unwrap()
    }

    /// Runs the shell's session to its refusal, as the frame's tick does.
    fn tick_to_refusal(shell: &mut Shell) {
        let presenter = shell.presenter.as_mut().unwrap();
        presenter.full_slices = true;
        presenter.next_paint = Some(std::time::Duration::ZERO);
        shell.tick(
            &egui::Context::default(),
            false,
            std::time::Duration::from_millis(20),
        );
    }

    /// Clicks the dialog button labelled `label` through two headless passes
    /// of the whole shell.
    fn click_dialog_button(
        shell: &mut Shell,
        host: &mut ui_kit::text_clipboard::fake::Host,
        label: &str,
    ) -> Vec<(String, egui::Rect)> {
        let mut frame = eframe::Frame::_new_kittest();
        host.pass(Vec::new(), |ui| eframe::App::ui(shell, ui, &mut frame));
        let painted = host.pass(Vec::new(), |ui| eframe::App::ui(shell, ui, &mut frame));
        let (_, button) = painted
            .iter()
            .rev()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("no {label} button among {painted:?}"));
        let at = button.center();
        host.click(at, |ui| eframe::App::ui(shell, ui, &mut frame));
        painted
    }

    /// A CPU refusal ends the session with the core's one-line error; the
    /// Machine error dialog shows the stop report under it, the report is
    /// saved in the machine's workspace and the status line says where, and
    /// Copy puts the message and the report on the clipboard.
    #[test]
    fn a_cpu_refusal_shows_saves_and_copies_its_stop_report() {
        use std::time::Duration;
        let temp = machine_file::test_support::TempDir::new("window-cpu-stop-report");
        let mut shell = refusing_shell(&temp);
        let ctx = egui::Context::default();
        shell.presenter.as_mut().unwrap().full_slices = true;
        shell.presenter.as_mut().unwrap().next_paint = Some(Duration::ZERO);
        shell.tick(&ctx, false, Duration::from_millis(20));
        let error = "cpu: unsupported cpu system operation LegacyInstruction at 002b:7c2b";
        assert_eq!(shell.dialog, Some(("Machine error", error.to_string())));
        let report = shell
            .error_report
            .clone()
            .expect("the refusal has a report");
        assert!(
            report.text.starts_with("cpu stop report\n"),
            "{}",
            report.text
        );
        assert!(report
            .text
            .contains("286 call gate, selector 0008, offset 7c23"));
        let saved = temp
            .path()
            .join("refusal.toml.local")
            .join(CPU_STOP_REPORT_DIR)
            .join(CPU_STOP_REPORT_FILE);
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), report.text);
        assert_eq!(
            report.saved,
            format!("cpu stop report saved to {}", saved.display())
        );
        assert_eq!(shell.message, format!("{error} — {}", report.saved));

        let mut host = ui_kit::text_clipboard::fake::Host::new("");
        let painted = click_dialog_button(&mut shell, &mut host, "Copy");
        assert!(painted.iter().any(|(text, _)| text == &report.text));
        assert_eq!(host.writes(), 1);
        assert_eq!(host.text(), format!("{error}\n\n{}", report.text));
        assert!(shell.dialog.is_some(), "Copy leaves the dialog open");

        // Stop shows the failure again with the same report, saved once.
        std::fs::remove_file(&saved).unwrap();
        assert!(!shell.stop());
        assert_eq!(shell.dialog, Some(("Machine error", error.to_string())));
        assert_eq!(shell.error_report, Some(report.clone()));
        assert!(!saved.exists());
        click_dialog_button(&mut shell, &mut host, "OK");
        assert_eq!(shell.dialog, None);

        // Any other failure shows no stale report.
        shell.show_error("storage failed".into());
        assert_eq!(shell.error_report, None);
        assert_eq!(shell.message, "storage failed");
    }

    /// A new session saves its own report: after the workspace was discarded,
    /// the same refusal writes the file again and the status line names a
    /// file that exists.
    #[test]
    fn a_new_session_saves_its_own_stop_report() {
        let temp = machine_file::test_support::TempDir::new("window-cpu-stop-again");
        let mut shell = refusing_shell(&temp);
        tick_to_refusal(&mut shell);
        assert!(!shell.stop());
        shell.dialog = None;
        let folder = temp.path().join("refusal.toml.local");
        std::fs::remove_dir_all(&folder).unwrap();

        shell.started(refusing_presenter());
        assert_eq!(shell.error_report, None);
        tick_to_refusal(&mut shell);
        let saved = folder.join(CPU_STOP_REPORT_DIR).join(CPU_STOP_REPORT_FILE);
        let report = shell.error_report.clone().unwrap();
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), report.text);
        assert!(shell
            .message
            .ends_with(&format!("cpu stop report saved to {}", saved.display())));
        assert!(!shell.stop());
    }

    /// The report's folder is one that clearing or discarding an image's
    /// workspace removes, so a discarded session leaves no report behind.
    #[test]
    fn clearing_a_workspace_removes_its_stop_report() {
        let fixture = crate::library_test_support::Fixture::new();
        let workspace = fixture.store.workspace("crashed-session").unwrap();
        let folder = workspace.path().join(CPU_STOP_REPORT_DIR);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(CPU_STOP_REPORT_FILE), "cpu stop report\n").unwrap();
        workspace.clear().unwrap();
        assert!(!folder.exists());
    }

    /// Every message dialog has Copy beside OK, which copies its text.
    #[test]
    fn a_message_dialog_copies_its_text() {
        let mut shell = shell();
        assert!(shell.stop());
        shell.dialog = Some(("Insert disk", "This machine has no floppy drive.".into()));
        let mut host = ui_kit::text_clipboard::fake::Host::new("before");
        click_dialog_button(&mut shell, &mut host, "Copy");
        assert_eq!(host.text(), "This machine has no floppy drive.");
        click_dialog_button(&mut shell, &mut host, "OK");
        assert_eq!(shell.dialog, None);
        assert_eq!(host.writes(), 1);
    }

    #[test]
    fn window_close_and_command_q_ask_while_running_and_other_actions_keep_the_session() {
        use KeyCode::*;
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        let close = || {
            let mut input = egui::RawInput::default();
            input
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
            input
        };
        for _ in 0..2 {
            let (commands, _) = pass(&ctx, close(), |ctx| shell.window_close(ctx));
            assert!(commands.contains(&egui::ViewportCommand::CancelClose));
            assert!(!commands.contains(&egui::ViewportCommand::Close));
            assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
            assert!(shell.presenter.is_some());
        }
        // Shortcuts wait while the question is open.
        shell.key(SuperLeft, Pressed, false, &ctx);
        shell.key(KeyK, Pressed, false, &ctx);
        shell.key(KeyK, Released, false, &ctx);
        assert!(shell.presenter.is_some());
        shell.answer_stop_prompt(false, &ctx);
        assert!(shell.presenter.as_ref().unwrap().running);

        logical_key(&mut shell, KeyQ, "q", Pressed, &ctx);
        logical_key(&mut shell, KeyQ, "q", Released, &ctx);
        shell.key(SuperLeft, Released, false, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(take_guest_scancodes(&mut shell).is_empty());
        shell.answer_stop_prompt(false, &ctx);

        // New machine builds an image beside the running machine and Import
        // image needs a stopped machine: neither ends the session.
        shell.command(Command::Placeholder("New machine"), &ctx);
        assert_eq!(shell.stop_prompt, None);
        assert!(shell.image_library.new_machine.is_some());
        shell.image_library.close_dialogs();
        shell.command(Command::ImportImage, &ctx);
        assert!(!shell.image_library.has_dialog());
        assert!(shell.presenter.as_ref().unwrap().running);

        let (commands, _) = pass(&ctx, close(), |ctx| {
            shell.window_close(ctx);
            shell.answer_stop_prompt(true, ctx);
        });
        assert!(commands.contains(&egui::ViewportCommand::Close));
        assert!(shell.presenter.is_none());
        assert!(shell.exit_requested);
    }

    #[test]
    fn stop_prompt_names_the_machine_and_escape_cancels() {
        let temp = machine_file::test_support::TempDir::new("stop-prompt-name");
        let path = temp.path().join("dos-games.toml");
        std::fs::write(
            &path,
            "schema=1\nname='DOS games'\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n",
        )
        .unwrap();
        let mut shell = shell();
        shell.library.push(LibraryEntry::pending(path));
        shell.selected = Some(0);
        let ctx = egui::Context::default();
        shell.command(Command::Library, &ctx);
        let mut text = Vec::new();
        for _ in 0..2 {
            text = pass(&ctx, Default::default(), |ctx| {
                assert_eq!(shell.stop_prompt_modal(ctx, false), None)
            })
            .1;
        }
        assert_eq!(shell.library[0].name, "DOS games");
        assert!(shows(
            &text,
            "unemu is running DOS games. Stop it and continue?"
        ));
        assert!(shows(&text, "Stop machine"));
        assert!(shows(&text, "Cancel"));
        let escape = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        let mut answer = None;
        pass(&ctx, escape, |ctx| {
            answer = shell.stop_prompt_modal(ctx, false)
        });
        assert_eq!(answer, Some(false));
        shell.answer_stop_prompt(false, &ctx);
        assert!(shell.presenter.is_some());
    }

    #[test]
    fn gaming_sessions_use_the_keep_or_discard_question_instead() {
        use crate::library_test_support::Fixture;
        let f = Fixture::new();
        let ctx = egui::Context::default();
        let mut shell = shell();
        shell.stop();
        shell.args.options.allow_unlisted_firmware = true;
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell.image_library.selected = Some(f.row.clone());
        shell.poll_images(&ctx);
        shell.command(Command::GamingMode, &ctx);
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.presenter.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(!shell.needs_stop_prompt());
        let (commands, _) = pass(&ctx, Default::default(), |ctx| {
            shell.command(Command::Exit, ctx)
        });
        assert_eq!(shell.stop_prompt, None);
        assert!(shell.presenter.is_none());
        assert!(shell.image_library.needs_decision());
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        // Quitting again while the question is open keeps the question.
        shell.macos_shortcuts = true;
        let (commands, _) = pass(&ctx, Default::default(), |ctx| {
            shell.key(KeyCode::SuperLeft, Pressed, false, ctx);
            logical_key(&mut shell, KeyCode::KeyQ, "q", Pressed, ctx);
        });
        assert!(shell.image_library.needs_decision());
        assert_eq!(shell.stop_prompt, None);
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        shell.image_library.shutdown();
    }

    #[test]
    fn insert_dialog_fixes_only_machine_file_policies_and_session_images_can_switch() {
        let temp = machine_file::test_support::TempDir::new("insert-write-policy");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("extra.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        draft.add_browsed(crate::media_dialog::browse(&path, draft.slot.geometry).unwrap());
        assert_eq!(draft.selected, Some(2));
        shell.confirm_insert();
        assert!(shell.insert_dialog.is_none());
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(2)
            )]
        );
        presenter.apply_media_events().unwrap();

        let dialog_text = |shell: &mut Shell, row: usize| {
            shell.command(Command::InsertFile(0), &ctx);
            shell.insert_dialog.as_mut().unwrap().select(Some(row));
            let mut text = Vec::new();
            for _ in 0..2 {
                text = pass(&ctx, Default::default(), |ctx| shell.media_modals(ctx)).1;
            }
            text
        };
        let browsed = dialog_text(&mut shell, 2);
        assert!(shows(&browsed, "Journal (keep changes separately)"));
        assert!(!shows(&browsed, "fixed by the machine file"));
        let declared = dialog_text(&mut shell, 0);
        assert!(shows(&declared, "Read-only — fixed by the machine file"));
        assert!(!shows(&declared, "Journal (keep changes separately)"));

        let draft = shell.insert_dialog.as_mut().unwrap();
        draft.select(Some(2));
        draft.journal = true;
        draft.journal_path = "extra.upfj".into();
        shell.confirm_insert();
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(3)
            )]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "extra.img"
        );
        let slot = presenter.media_slots[0].as_ref().unwrap();
        assert_eq!(slot.entries.len(), 3);
        assert_eq!(
            slot.entries[2].medium.write,
            assembly::WriteMode::journal(root.join("extra.upfj"))
        );
        let journaled = dialog_text(&mut shell, 2);
        assert!(shows(&journaled, "Separate journal path"));
        assert!(shell.insert_dialog.as_ref().unwrap().journal);
        assert!(shell.stop());
    }

    /// What one headless status-bar pass painted and asked for.
    struct StatusPass {
        text: Vec<(String, egui::Rect)>,
        /// Filled rectangles, so a drive light can be found by its colour.
        fills: Vec<(Color32, egui::Rect)>,
        /// Accessibility labels of the pass's widgets.
        labels: Vec<String>,
        commands: Vec<Command>,
        /// Texts cut short with an ellipsis; a galley keeps its whole text.
        elided: Vec<String>,
        /// The label of the widget with the keyboard focus.
        focused: Option<String>,
    }

    const AMBER: Color32 = Color32::from_rgb(255, 178, 31);

    fn status_pass(
        ctx: &egui::Context,
        shell: &mut Shell,
        width: f32,
        events: Vec<egui::Event>,
    ) -> StatusPass {
        ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(width, 400.0),
            )),
            events,
            ..Default::default()
        };
        let mut commands = Vec::new();
        // The same bottom panel as the running shell, margins included.
        let mut output = ctx.run_ui(input, |ui| {
            egui::Panel::bottom("statusbar").show(ui, |ui| shell.status(ui, &mut commands));
        });
        output.textures_delta.clear();
        let mut text = Vec::new();
        let mut fills = Vec::new();
        let mut elided = Vec::new();
        for shape in &output.shapes {
            match &shape.shape {
                egui::epaint::Shape::Text(shown) => {
                    if shown.galley.elided {
                        elided.push(shown.galley.text().to_owned());
                    }
                    text.push((
                        shown.galley.text().to_owned(),
                        shown.galley.rect.translate(shown.pos.to_vec2()),
                    ));
                }
                egui::epaint::Shape::Rect(rect) => fills.push((rect.fill, rect.rect)),
                _ => {}
            }
        }
        let update = output.platform_output.accesskit_update.take();
        let labels = update
            .as_ref()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| node.label().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let focused = ctx.memory(|memory| memory.focused()).and_then(|id| {
            update
                .as_ref()?
                .nodes
                .iter()
                .find(|(node, _)| *node == id.accesskit_id())
                .and_then(|(_, node)| node.label().map(str::to_owned))
        });
        output.drop_without_applying_deltas();
        StatusPass {
            text,
            fills,
            labels,
            commands,
            elided,
            focused,
        }
    }

    /// Press and release at `at`; returns what both passes asked for.
    fn click_status(
        ctx: &egui::Context,
        shell: &mut Shell,
        width: f32,
        at: egui::Pos2,
    ) -> Vec<Command> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut commands = status_pass(
            ctx,
            shell,
            width,
            vec![egui::Event::PointerMoved(at), button(true)],
        )
        .commands;
        commands.extend(status_pass(ctx, shell, width, vec![button(false)]).commands);
        commands
    }

    fn text_starting(pass: &StatusPass, prefix: &str) -> Option<(String, egui::Rect)> {
        pass.text
            .iter()
            .find(|(shown, _)| shown.starts_with(prefix))
            .cloned()
    }

    fn busy_shell() -> Shell {
        let mut shell = shell();
        shell.presenter = Some(
            Presenter::new(
                &crate::tests::busy_drives_config(),
                assembly::AssemblyOptions::default(),
                &mut io::sink(),
            )
            .unwrap(),
        );
        shell
    }

    #[test]
    fn drive_chips_appear_only_while_running_and_never_move_the_other_cells() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let wide = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(&wide.text, "A: first.img · 1.44M · read-only"));
        assert_eq!(text_starting(&wide, "B:"), None, "the machine has no B:");
        let cells = |pass: &StatusPass| -> Vec<f32> {
            ["Power", "CPU unknown", "EMS unknown", "Speed"]
                .map(|label| text_starting(pass, label).unwrap().1.left())
                .to_vec()
        };
        assert!(wide.elided.is_empty());
        let running = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        let (_, rect) = text_starting(&running, "A: first.img").unwrap();
        let cpu = text_starting(&running, "CPU unknown").unwrap().1;
        assert!(rect.right() <= cpu.left(), "the chip stays inside its area");
        shell
            .presenter
            .as_mut()
            .unwrap()
            .assembled()
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let ejected = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        assert!(shows(&ejected.text, "A: empty"));
        assert!(shell.stop());
        let stopped = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        assert_eq!(text_starting(&stopped, "A:"), None);
        assert_eq!(cells(&running), cells(&ejected));
        assert_eq!(cells(&running), cells(&stopped));
    }

    #[test]
    fn default_width_keeps_the_pointer_release_hint_readable() {
        let ctx = egui::Context::default();
        let mut shell = shell();
        let hint = "Pointer captured — Ctrl+Alt+G releases";
        shell.message = hint.into();
        let mut entry = LibraryEntry::pending(std::path::PathBuf::from("machine.toml"));
        entry.rate = "80286 · 32/39".into();
        entry.ems = "EMS 2048 KiB".into();
        shell.library.push(entry);
        shell.selected = Some(0);
        let pass = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        for text in [hint, "80286 · 32/39", "EMS 2048 KiB", "Speed 100%"] {
            label_rect(&pass.text, text);
            assert!(!pass.elided.iter().any(|shown| shown == text), "{text}");
        }
    }

    #[test]
    fn default_width_chips_shorten_only_the_name_and_keep_size_and_mode() {
        let ctx = egui::Context::default();
        let mut shell = busy_shell();
        let wide = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(&wide.text, "A: disk 1 · 1.44M · volatile"));
        assert!(shows(&wide.text, "C: inline image · read-only"));
        let pass = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        let (a, _) = text_starting(&pass, "A: ").unwrap();
        let (c, _) = text_starting(&pass, "C: ").unwrap();
        assert!(a.ends_with("… · 1.44M · volatile"), "{a}");
        assert!(c.ends_with("… · read-only"), "{c}");
        assert!(pass.elided.is_empty(), "{:?}", pass.elided);
        // The full line stays available to the hover and to accessibility.
        assert!(pass
            .labels
            .iter()
            .any(|label| label == "Drive A: disk 1 · 1.44M · volatile"));
        let mut shell = media_shell();
        let pass = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        assert!(shows(&pass.text, "A: first.img · 1.44M · read-only"));
        assert!(shell.stop());
    }

    #[test]
    fn empty_drive_has_no_eject_in_its_chip_menu_or_the_media_menu() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell
            .presenter
            .as_mut()
            .unwrap()
            .assembled()
            .change_floppy(assembly::DeviceId::Fdc, 0, assembly::FloppyAction::Eject)
            .unwrap();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: empty").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(&menu.text, "Drive A: (1.44M) — empty"));
        assert!(!shows(&menu.text, "Write mode"));
        let eject = label_rect(&menu.text, "Eject A:").center();
        assert!(click_status(&ctx, &mut shell, 1600.0, eject).is_empty());
        // Any click closes the menu; the enabled items still act once reopened.
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let insert = label_rect(&menu.text, "Insert…").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, insert),
            [Command::Insert(0)]
        );

        let open_media = |shell: &mut Shell| {
            egui::Popup::close_all(&ctx);
            let drawn = draw(&ctx, shell, Vec::new());
            click(&ctx, shell, label_rect(&drawn.text, "Media").center());
            draw(&ctx, shell, Vec::new()).text
        };
        let text = open_media(&mut shell);
        assert!(shows(&text, "Drive A: (1.44M) — empty"));
        let eject = label_rect(&text, "Eject A:").center();
        assert!(click(&ctx, &mut shell, eject).is_empty());
        let text = open_media(&mut shell);
        let next = label_rect(&text, "Next disk in catalogue").center();
        assert_eq!(click(&ctx, &mut shell, next), [Command::Next(0)]);
        assert!(shell.stop());
    }

    #[test]
    fn chip_insert_opens_the_media_picker_and_its_set_reads_the_library_store() {
        let temp = machine_file::test_support::TempDir::new("chip-from-library");
        let root = temp.path().canonicalize().unwrap();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        // A store path that does not exist: the user's own store is never read.
        let store = root.join("no-store");
        shell.preferences.store_root = Some(store.clone());
        shell.args.machine = root.join("machine.toml");
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: first.img · 1.44M · read-only").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(!shows(&menu.text, "From library…"));
        let insert = label_rect(&menu.text, "Insert…").center();
        let commands = click_status(&ctx, &mut shell, 1600.0, insert);
        assert_eq!(commands, [Command::Insert(0)]);
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.as_ref().expect("the media picker");
        assert_eq!(picker.unit, 0);
        assert_eq!(picker.source, crate::media_picker::Source::Library);
        shell.apply_picker(crate::media_picker::tests::set_outcome(0));
        let draft = shell.insert_dialog.as_mut().unwrap();
        assert!(draft.automatic);
        assert_eq!(draft.unit, 0);
        let library = draft.library.as_mut().expect("the library view is open");
        assert_eq!(library.store_path, store.to_string_lossy());
        assert_eq!(
            library.workspace,
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap()
        );
        let mut failure = None;
        for _ in crate::test_wait::rounds() {
            failure = library.poll();
            if failure.is_some() || !library.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(failure.is_some(), "a missing store reports an error");
        shell.insert_dialog = None;
        assert!(shell.stop());
    }

    /// Create from files… of a drive chip's menu, and of the Media menu
    /// under each drive's Insert…, opens the media picker straight in its
    /// Create view for that drive.
    #[test]
    fn create_from_files_in_the_menus_opens_the_create_view_on_that_drive() {
        use crate::media_picker::Mode;
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: first.img · 1.44M · read-only").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let create = label_rect(&menu.text, "Create from files…").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, create),
            [Command::CreateFromFiles(0)]
        );
        shell.command(Command::CreateFromFiles(0), &ctx);
        let picker = shell.picker.as_ref().expect("the media picker");
        assert_eq!((picker.unit, picker.mode), (0, Mode::Create));
        shell.picker = None;
        assert!(shell.stop());

        let temp = machine_file::test_support::TempDir::new("shell-create-menu");
        let (mut shell, _) = stopped_machine_file_shell(&temp);
        let ctx = egui::Context::default();
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Media").center());
        let text = draw(&ctx, &mut shell, Vec::new()).text;
        let rows = |label: &str| {
            let mut rows: Vec<egui::Rect> = text
                .iter()
                .filter(|(shown, _)| shown == label)
                .map(|(_, rect)| *rect)
                .collect();
            rows.sort_by(|a, b| a.top().total_cmp(&b.top()));
            rows
        };
        let (inserts, creates) = (rows("Insert…"), rows("Create from files…"));
        assert_eq!(creates.len(), 2, "one under each drive's Insert…");
        for (insert, create) in inserts.iter().zip(&creates) {
            assert!(create.top() > insert.top() && create.top() - insert.bottom() < 12.0);
        }
        assert_eq!(
            click(&ctx, &mut shell, creates[1].center()),
            [Command::CreateFromFiles(1)]
        );
        shell.command(Command::CreateFromFiles(1), &ctx);
        let picker = shell.picker.as_ref().expect("the media picker");
        assert_eq!((picker.unit, picker.mode), (1, Mode::Create));
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// The texts of an open Catalogue submenu, from its heading on: the
    /// submenu is the topmost layer, so its texts come last.
    fn submenu_texts(text: &[(String, egui::Rect)]) -> Vec<String> {
        let start = text
            .iter()
            .position(|(shown, _)| {
                shown.contains("'s catalogue")
                    || (shown.starts_with("For ") && shown.contains("at the next Start"))
            })
            .unwrap_or_else(|| panic!("no Catalogue submenu in {text:?}"));
        text[start..]
            .iter()
            .map(|(shown, _)| shown.clone())
            .collect()
    }

    /// The Catalogue items of the open menu, top to bottom.
    fn catalogue_items(text: &[(String, egui::Rect)]) -> Vec<(String, egui::Rect)> {
        let mut items: Vec<(String, egui::Rect)> = text
            .iter()
            .filter(|(shown, _)| shown.starts_with("Catalogue"))
            .cloned()
            .collect();
        items.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
        items
    }

    /// Open the Media menu, then drive `unit`'s Catalogue submenu by
    /// hovering its item; the frame that shows it.
    fn media_catalogue(ctx: &egui::Context, shell: &mut Shell, unit: usize) -> Drawn {
        egui::Popup::close_all(ctx);
        let drawn = draw(ctx, shell, Vec::new());
        click(ctx, shell, label_rect(&drawn.text, "Media").center());
        let text = draw(ctx, shell, Vec::new()).text;
        let at = catalogue_items(&text)[unit].1.center();
        let mut drawn = draw(ctx, shell, vec![egui::Event::PointerMoved(at)]);
        for _ in 0..3 {
            drawn = draw(ctx, shell, vec![egui::Event::PointerMoved(at)]);
        }
        drawn
    }

    /// Media › Catalogue lists the drive's catalogue in order under a line
    /// with the drive and the count, ● and "in A:" on the disk in the drive,
    /// after Create from files… and before Next disk. A chosen disk goes in
    /// now, as This machine's Insert puts it in, and the status bar says so;
    /// while halted it adds that Ctrl+Alt+Del boots from it.
    #[test]
    fn media_catalogue_puts_a_disk_into_the_running_drive_as_this_machine_does() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "A:'s catalogue, in order · 2 disks",
                "●",
                "first.img",
                "in A:",
                "second.img"
            ]
        );
        assert!(drawn.toggled.contains(&("first.img".to_owned(), true)));
        assert!(drawn.toggled.contains(&("second.img".to_owned(), false)));
        let items = catalogue_items(&drawn.text);
        assert_eq!(items[0].0, "Catalogue");
        let top = |label: &str| {
            let mut rows: Vec<f32> = drawn
                .text
                .iter()
                .filter(|(shown, _)| shown == label)
                .map(|(_, rect)| rect.top())
                .collect();
            rows.sort_by(f32::total_cmp);
            rows[0]
        };
        assert!(top("Create from files…") < items[0].1.top());
        assert!(items[0].1.top() < top("Next disk in catalogue"));
        let second = label_rect(&drawn.text, "second.img").center();
        assert_eq!(
            click(&ctx, &mut shell, second),
            [Command::CatalogueDisk(0, 1)]
        );
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        let chosen = std::mem::take(&mut shell.presenter.as_mut().unwrap().media_events);
        assert_eq!(
            chosen,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(1)
            )]
        );
        assert_eq!(shell.message, "A: now holds second.img.");
        // Insert of This machine lists the same disks and asks for the same.
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.as_mut().unwrap();
        picker.set_source(crate::media_picker::Source::Machine);
        let names: Vec<String> = picker.machine().drives[0]
            .catalogue
            .iter()
            .map(|disk| disk.name.clone())
            .collect();
        assert_eq!(names, ["first.img", "second.img"]);
        picker.choose(crate::media_picker::Choice::Entry(0, 1));
        let outcome = picker.insert().unwrap();
        shell.apply_picker(outcome);
        assert_eq!(shell.presenter.as_ref().unwrap().media_events, chosen);
        shell.presenter.as_mut().unwrap().media_events.clear();
        shell.presenter.as_mut().unwrap().halted = true;
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            shell.message,
            "A: now holds second.img. Press Ctrl+Alt+Del to boot from it."
        );
        assert_eq!(shell.presenter.as_ref().unwrap().media_events, chosen);
        shell.presenter.as_mut().unwrap().halted = false;
        assert!(shell.stop());
    }

    /// A stopped shell with one machine file selected: its 1.44M A: holds
    /// `one.img` at power-on and `two.img` too in its catalogue; its B:
    /// starts empty, with no disk in its catalogue.
    fn stopped_two_disk_shell(temp: &machine_file::test_support::TempDir) -> Shell {
        std::fs::write(temp.path().join("bios.bin"), vec![0xF4; 32768]).unwrap();
        for name in ["one.img", "two.img"] {
            std::fs::write(temp.path().join(name), vec![0; 1_474_560]).unwrap();
        }
        let path = temp.path().join("machine.toml");
        std::fs::write(&path, "schema=1\n[board]\nmodel='ibm-5170'\n[firmware.bios]\nimage='bios.bin'\nprovides=['floppy']\n[[card]]\nkind='mda'\n[[card]]\nkind='fdc-765'\n[[card.drives]]\nunit=0\ntype='1.44M'\n[[card.drives.media]]\nimage='one.img'\nwrite='readonly'\n[[card.drives.media]]\nimage='two.img'\nwrite='readonly'\n[[card.drives]]\nunit=1\ntype='1.44M'\nempty=true\n").unwrap();
        let mut shell = shell();
        shell.presenter = None;
        shell.args.options.allow_unlisted_firmware = true;
        // A store path that does not exist: the user's own store is never read.
        shell.preferences.store_root = Some(temp.path().join("no-store"));
        shell.library = vec![LibraryEntry::inspect_with_rom_dirs(
            path,
            shell.args.options,
            &[],
        )];
        shell.selected = Some(0);
        shell.refresh_library();
        finish_readiness(&mut shell);
        shell.sync_pending_media();
        shell
    }

    /// On a stopped machine the Catalogue submenu marks the disk its drive
    /// holds at power-on. A chosen disk waits for the next Start, as This
    /// machine makes it wait, and says "at Start"; choosing the disk in the
    /// drive takes the waiting one out again. A waiting disk that is not in
    /// the catalogue yet is named under the list. An empty catalogue
    /// disables the item with the reason.
    #[test]
    fn a_stopped_machine_takes_a_catalogue_disk_at_its_next_start() {
        let temp = machine_file::test_support::TempDir::new("shell-catalogue-stopped");
        let mut shell = stopped_two_disk_shell(&temp);
        let name = shell.stopped_name();
        let ctx = egui::Context::default();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            catalogue_items(&drawn.text)
                .into_iter()
                .map(|(shown, _)| shown)
                .collect::<Vec<_>>(),
            ["Catalogue", "Catalogue · empty"]
        );
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "A:'s catalogue, in order · 2 disks",
                "●",
                "one.img",
                "in A:",
                "two.img"
            ]
        );
        let two = label_rect(&drawn.text, "two.img").center();
        assert_eq!(click(&ctx, &mut shell, two), [Command::CatalogueDisk(0, 1)]);
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            shell.message,
            format!("two.img goes into A: of {name} at its next Start.")
        );
        let chips = |shell: &mut Shell| -> Vec<String> {
            shell
                .drive_views()
                .iter()
                .map(crate::drive_chips::DriveView::chip_text)
                .collect()
        };
        assert_eq!(
            chips(&mut shell),
            ["A: two.img · 1.44M · read-only · at Start"]
        );
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "A:'s catalogue, in order · 2 disks",
                "●",
                "one.img",
                "in A:",
                "two.img",
                "at Start"
            ]
        );
        let one = label_rect(&drawn.text, "one.img").center();
        assert_eq!(click(&ctx, &mut shell, one), [Command::CatalogueDisk(0, 0)]);
        shell.command(Command::CatalogueDisk(0, 0), &ctx);
        assert_eq!(
            shell.message,
            "Took the pending disk out; one.img stays in A:."
        );
        assert!(chips(&mut shell).is_empty());
        shell.command(Command::CatalogueDisk(0, 0), &ctx);
        assert_eq!(shell.message, "one.img is in A: already.");
        // A file of this computer waits for Start outside the catalogue.
        let other = temp.path().join("other.img");
        std::fs::write(&other, vec![0; 737_280]).unwrap();
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        let browsed = crate::media_dialog::browse(&other, draft.slot.geometry).unwrap();
        draft.add_browsed(browsed);
        shell.confirm_insert();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        let texts = submenu_texts(&drawn.text);
        assert_eq!(
            texts.last().map(String::as_str),
            Some("At Start: other.img, which joins the catalogue then.")
        );
        assert!(!texts.iter().any(|text| text == "at Start"));
        // Choosing a catalogue disk replaces the waiting file.
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            chips(&mut shell),
            ["A: two.img · 1.44M · read-only · at Start"]
        );
    }

    /// One disk or none disables the item, which says why.
    #[test]
    fn one_disk_or_none_disables_catalogue_with_the_reason() {
        let temp = machine_file::test_support::TempDir::new("shell-catalogue-one");
        let (mut shell, _) = stopped_machine_file_shell(&temp);
        let ctx = egui::Context::default();
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Media").center());
        let text = draw(&ctx, &mut shell, Vec::new()).text;
        assert_eq!(
            catalogue_items(&text)
                .into_iter()
                .map(|(shown, _)| shown)
                .collect::<Vec<_>>(),
            ["Catalogue · one disk", "Catalogue · empty"]
        );
        for (unit, why) in [
            (0, "Only one disk in A:'s catalogue. Insert… adds more."),
            (
                1,
                "B:'s catalogue is empty. Insert… and Create from files… add disks to it.",
            ),
        ] {
            let catalogue = shell.catalogue(unit, [None, None]).unwrap();
            assert_eq!(
                catalogue.disabled().map(|(_, reason)| reason).as_deref(),
                Some(why)
            );
        }
        // A disabled item opens nothing.
        let one = catalogue_items(&text)[0].1.center();
        for _ in 0..3 {
            let drawn = draw(&ctx, &mut shell, vec![egui::Event::PointerMoved(one)]);
            assert!(!shows(&drawn.text, "'s catalogue"));
        }
        assert!(click(&ctx, &mut shell, one).is_empty());
    }

    /// A running machine with `count` disks in A:'s catalogue.
    fn many_disk_shell(count: usize) -> Shell {
        let mut shell = shell();
        let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: (1..=count)
                .map(|number| assembly::FloppyMedium {
                    image: assembly::ImageSource::Inline(vec![0; 1_474_560]),
                    expected_sha256: None,
                    write: assembly::WriteMode::ReadOnly,
                    name: format!("AZURAMON {number} of {count}"),
                })
                .collect(),
            inserted: 0,
        });
        shell.presenter = Some(
            Presenter::new(
                &config,
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
                &mut io::sink(),
            )
            .unwrap(),
        );
        shell
    }

    /// Past twelve disks the submenu lists twelve, two before the disk in
    /// the drive and the rest after it, and All N disks…, which opens Insert…
    /// on This machine with that disk chosen and scrolled into view.
    #[test]
    fn a_long_catalogue_lists_twelve_disks_and_all_of_them_in_the_picker() {
        use crate::media_picker::{Choice, Source};
        let ctx = egui::Context::default();
        let mut shell = many_disk_shell(13);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .assembled()
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(5),
            )
            .unwrap();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        let texts = submenu_texts(&drawn.text);
        assert_eq!(texts[0], "Disks 2–13 of 13 in A:'s catalogue");
        let disks: Vec<&String> = texts.iter().filter(|t| t.starts_with("AZURAMON")).collect();
        assert_eq!(disks.len(), 12);
        assert_eq!(disks[0], "AZURAMON 2 of 13");
        assert_eq!(disks[11], "AZURAMON 13 of 13");
        let here = texts.iter().position(|t| t == "●").unwrap();
        assert_eq!(texts[here + 1], "AZURAMON 6 of 13");
        assert_eq!(
            texts[texts.len() - 2..],
            ["All 13 disks…", "Insert… › This machine"]
        );
        let all = label_rect(&drawn.text, "All 13 disks…").center();
        assert_eq!(click(&ctx, &mut shell, all), [Command::CatalogueAll(0)]);
        shell.command(Command::CatalogueAll(0), &ctx);
        let picker = shell.picker.as_ref().expect("the media picker");
        assert_eq!((picker.unit, picker.source), (0, Source::Machine));
        assert_eq!(picker.choice, Some(Choice::Entry(0, 5)));
        assert!(picker.reveal);
        shell.picker = None;
        assert!(shell.stop());
    }

    /// The keyboard reaches a drive's catalogue without a new key: ↓ to
    /// Catalogue, → or Enter opens it on the disk in the drive, ↓ and Enter
    /// put the next disk in. ← and Escape go back to the item with the menu
    /// still open, and Escape there closes the menu.
    #[test]
    fn the_keyboard_reaches_the_catalogue_through_the_media_menu() {
        use egui::Key::{ArrowDown, ArrowLeft, ArrowRight, Enter, Escape};
        let ctx = egui::Context::default();
        let mut shell = many_disk_shell(3);
        // The disk in the drive is not the first, so → shows where it goes.
        shell
            .presenter
            .as_mut()
            .unwrap()
            .assembled()
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(1),
            )
            .unwrap();
        let open = |shell: &mut Shell| {
            egui::Popup::close_all(&ctx);
            let drawn = draw(&ctx, shell, Vec::new());
            click(&ctx, shell, label_rect(&drawn.text, "Media").center());
            let mut drawn = draw(&ctx, shell, Vec::new());
            for _ in 0..6 {
                if drawn.focused.as_deref() == Some("Catalogue") {
                    break;
                }
                draw(&ctx, shell, vec![key(ArrowDown)]);
                drawn = draw(&ctx, shell, Vec::new());
            }
            assert_eq!(drawn.focused.as_deref(), Some("Catalogue"));
        };
        // A key, then two frames to settle; every command the three asked for.
        let press = |shell: &mut Shell, pressed: egui::Key| {
            let mut commands = draw(&ctx, shell, vec![key(pressed)]).commands;
            commands.extend(draw(&ctx, shell, Vec::new()).commands);
            let drawn = draw(&ctx, shell, Vec::new());
            Drawn {
                commands: [commands, drawn.commands].concat(),
                ..drawn
            }
        };
        let heading = "A:'s catalogue, in order · 3 disks";
        open(&mut shell);
        let drawn = press(&mut shell, ArrowRight);
        assert!(shows(&drawn.text, heading));
        assert_eq!(
            drawn.focused.as_deref(),
            Some("AZURAMON 2 of 3"),
            "the disk in the drive"
        );
        let drawn = press(&mut shell, ArrowDown);
        assert_eq!(drawn.focused.as_deref(), Some("AZURAMON 3 of 3"));
        let drawn = press(&mut shell, Enter);
        assert_eq!(drawn.commands, [Command::CatalogueDisk(0, 2)]);
        assert!(!shows(&drawn.text, heading), "choosing closes the menus");
        assert!(!shows(&drawn.text, "Next disk in catalogue"));
        open(&mut shell);
        press(&mut shell, ArrowRight);
        let drawn = press(&mut shell, ArrowLeft);
        assert_eq!(drawn.focused.as_deref(), Some("Catalogue"));
        assert!(!shows(&drawn.text, heading));
        assert!(shows(&drawn.text, "Next disk in catalogue"));
        let drawn = press(&mut shell, Enter);
        assert_eq!(drawn.focused.as_deref(), Some("AZURAMON 2 of 3"));
        assert!(drawn.commands.is_empty());
        let drawn = press(&mut shell, Escape);
        assert_eq!(drawn.focused.as_deref(), Some("Catalogue"));
        assert!(!shows(&drawn.text, heading));
        assert!(
            shows(&drawn.text, "Next disk in catalogue"),
            "the menu stays open"
        );
        let drawn = press(&mut shell, Escape);
        assert!(!shows(&drawn.text, "Next disk in catalogue"));
        assert!(drawn.commands.is_empty());
        assert!(shell.stop());
    }

    /// Open the chip of drive A: and its Catalogue submenu by hovering its
    /// item, in a status bar `width` wide; the frames that follow, hovering.
    fn chip_catalogue(
        ctx: &egui::Context,
        shell: &mut Shell,
        width: f32,
        frames: usize,
    ) -> Vec<StatusPass> {
        egui::Popup::close_all(ctx);
        let pass = status_pass(ctx, shell, width, Vec::new());
        let chip = text_starting(&pass, "A: ").expect("the A: chip").1.center();
        click_status(ctx, shell, width, chip);
        let menu = status_pass(ctx, shell, width, Vec::new());
        let item = label_rect(&menu.text, "Catalogue").center();
        (0..frames)
            .map(|_| status_pass(ctx, shell, width, vec![egui::Event::PointerMoved(item)]))
            .collect()
    }

    /// A drive chip's menu has the same Catalogue submenu as the Media menu,
    /// with the same items, and a disk chosen there goes in the same way.
    #[test]
    fn the_chip_and_the_media_menu_show_the_same_catalogue() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let media = submenu_texts(&media_catalogue(&ctx, &mut shell, 0).text);
        let chip = chip_catalogue(&ctx, &mut shell, 1600.0, 4).pop().unwrap();
        assert_eq!(submenu_texts(&chip.text), media);
        let create = label_rect(&chip.text, "Create from files…");
        let item = label_rect(&chip.text, "Catalogue");
        assert!(create.top() < item.top() && item.top() - create.bottom() < 12.0);
        let second = label_rect(&chip.text, "second.img").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, second),
            [Command::CatalogueDisk(0, 1)]
        );
        assert!(shell.stop());
    }

    /// In every skin and theme the chip's Catalogue submenu grows upward
    /// from the status bar, stays inside the window, shows the in-drive
    /// mark, and keeps its place and size from frame to frame.
    #[test]
    fn the_chip_catalogue_holds_still_above_the_status_bar_in_every_look() {
        for (classic, dark) in [(true, false), (true, true), (false, false), (false, true)] {
            let what = format!("classic {classic}, dark {dark}");
            let ctx = egui::Context::default();
            ctx.set_visuals(skin_visuals(classic, dark));
            let mut shell = media_shell();
            shell.classic = classic;
            let frames = chip_catalogue(&ctx, &mut shell, 1040.0, 8);
            let submenu = |pass: &StatusPass| -> Vec<(String, egui::Rect)> {
                let start = pass
                    .text
                    .iter()
                    .position(|(shown, _)| shown.contains("'s catalogue"))
                    .unwrap_or_else(|| panic!("{what}: no submenu in {:?}", pass.text));
                pass.text[start..].to_vec()
            };
            let settled = submenu(&frames[2]);
            for pass in &frames[2..] {
                assert_eq!(submenu(pass), settled, "{what}: the submenu moves or grows");
            }
            let names: Vec<&str> = settled.iter().map(|(shown, _)| shown.as_str()).collect();
            assert_eq!(
                names,
                [
                    "A:'s catalogue, in order · 2 disks",
                    "●",
                    "first.img",
                    "in A:",
                    "second.img"
                ],
                "{what}"
            );
            let chip = text_starting(&frames[7], "A: first.img").unwrap().1;
            let window = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1040.0, 400.0));
            for (shown, rect) in &settled {
                assert!(
                    rect.bottom() <= chip.top(),
                    "{what}: {shown} {rect:?} {chip:?}"
                );
                assert!(window.contains_rect(*rect), "{what}: {shown} {rect:?}");
            }
            let item = label_rect(&frames[7].text, "Catalogue");
            let last = settled.last().unwrap().1;
            assert!(
                (last.bottom() - item.bottom()).abs() < 8.0 && settled[0].1.top() < item.top(),
                "{what}: the submenu grows upward from its item: {settled:?} {item:?}"
            );
            assert!(shell.stop());
        }
    }

    /// Import a three-disk 360K installation set titled "Win 3.1", tag
    /// `win:3.1`, its first disk labelled "Disk 1 (Setup)"; its manifest.
    fn import_win_set(f: &crate::library_test_support::Fixture) -> image_store::Descriptor {
        let disks: Vec<_> = (1..=3)
            .map(|number| {
                let disk = f.temp.path().join(format!("win-{number}.img"));
                let create: Vec<std::ffi::OsString> = vec![
                    "floppy".into(),
                    "--type".into(),
                    "360K".into(),
                    "--format".into(),
                    "--label".into(),
                    format!("WIN{number}").into(),
                    disk.clone().into_os_string(),
                ];
                pc_image::media_create::execute(
                    pc_image::media_create::parse(&create).unwrap(),
                    &mut std::io::sink(),
                )
                .unwrap();
                disk
            })
            .collect();
        use crate::library_test_support::{import_media_set, Medium};
        import_media_set(
            &f.store,
            "win:3.1-r1",
            "Win 3.1",
            &[
                Medium::new(&disks[0]).label("Disk 1 (Setup)"),
                Medium::new(&disks[1]).label("Disk 2"),
                Medium::new(&disks[2]).label("Disk 3"),
            ],
            &[],
        );
        f.store.tag_descriptor("win:3.1").unwrap()
    }

    /// A stopped library machine knows its own disks only at Start: with
    /// nothing waiting the item says so, never "empty". A set waiting for
    /// Start lists its disks, the chosen one "at Start", and names the
    /// image's own disks under them; choosing another rewrites the pending
    /// insert. The chip, the Insert status and the picker's drive button
    /// name the waiting disk as the submenu does.
    #[test]
    fn a_stopped_library_machine_lists_the_set_disks_waiting_for_start() {
        use crate::catalogue_menu::OWN_DISKS_AT_START;
        let f = crate::library_test_support::Fixture::new();
        let set = import_win_set(&f);
        let (mut shell, id) = machine_shell(&f);
        shell
            .image_library
            .pump_until(|view| view.picker_set(&set.digest).is_some());
        let ids: Vec<String> = shell
            .image_library
            .picker_set(&set.digest)
            .unwrap()
            .disks
            .iter()
            .map(|disk| disk.id.clone())
            .collect();
        assert_eq!(ids.len(), 3);
        let ctx = egui::Context::default();
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Media").center());
        let text = draw(&ctx, &mut shell, Vec::new()).text;
        assert_eq!(catalogue_items(&text)[0].0, "Catalogue · known at Start");
        assert_eq!(
            shell.catalogue(0, [None, None]).unwrap().disabled(),
            Some((
                "Catalogue · known at Start".into(),
                IMAGE_DISKS_AT_START.into()
            ))
        );
        let insert = |medium: &str| image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: set.clone(),
            reference: "win:3.1".into(),
            medium: medium.into(),
            all: true,
            journal: false,
        };
        shell.apply_picker(crate::media_picker::Outcome::Pending {
            id: id.clone(),
            insert: insert(&ids[0]),
        });
        let waiting = "Win 3.1 · 1 of 3 — Setup (all 3 disks join the catalogue)";
        assert_eq!(shell.image_library.pending_label(&insert(&ids[0])), waiting);
        assert_eq!(
            shell.message,
            format!("{waiting} goes into A: of DOS at its next Start")
        );
        let chips = |shell: &mut Shell| -> Vec<String> {
            shell
                .drive_views()
                .iter()
                .map(crate::drive_chips::DriveView::chip_text)
                .collect()
        };
        assert_eq!(
            chips(&mut shell),
            [format!("A: {waiting} · 360K · read-only · at Start")]
        );
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.take().expect("the media picker");
        assert_eq!(picker.drive().unwrap().holds, format!("{waiting} at Start"));
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "For A: at the next Start, in order · 3 disks",
                "Win 3.1 · 1 of 3 — Setup",
                "at Start",
                "Win 3.1 · 2 of 3",
                "Win 3.1 · 3 of 3",
                OWN_DISKS_AT_START
            ]
        );
        let second = label_rect(&drawn.text, "Win 3.1 · 2 of 3").center();
        assert_eq!(
            click(&ctx, &mut shell, second),
            [Command::CatalogueDisk(0, 1)]
        );
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            shell.message,
            "Win 3.1 · 2 of 3 goes into A: of DOS at its next Start."
        );
        assert_eq!(
            chips(&mut shell),
            ["A: Win 3.1 · 2 of 3 (all 3 disks join the catalogue) · 360K · read-only · at Start"]
        );
        // The record changes on the worker, and the listing after it agrees.
        shell.image_library.pump_until(|view| {
            pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending
                == [insert(&ids[1])]
                && view
                    .selected_machine_row()
                    .is_some_and(|row| row.listing.machine.pending == [insert(&ids[1])])
        });
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        let texts = submenu_texts(&drawn.text);
        let marked = texts.iter().position(|text| text == "at Start").unwrap();
        assert_eq!(texts[marked - 1], "Win 3.1 · 2 of 3");
    }

    /// Disks Create from files… made for a stopped library machine wait for
    /// its Start the same way: listed, disk 1 "at Start" and on the chip,
    /// and choosing another makes it the one that goes in.
    #[test]
    fn a_stopped_library_machine_lists_the_created_disks_waiting_for_start() {
        use crate::created_media::{CreateOrder, Place};
        use media::fat12::{volume_label, FloppyFormat, HostItem, ShortName};
        let f = crate::library_test_support::Fixture::new();
        let (mut shell, id) = machine_shell(&f);
        let game = f.temp.path().join("host/game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("one.dat"), vec![1; 250_000]).unwrap();
        std::fs::write(game.join("two.dat"), vec![2; 250_000]).unwrap();
        shell
            .image_library
            .send(crate::library_worker::Request::CreateDisks(
                Box::new(CreateOrder {
                    unit: 0,
                    items: vec![HostItem::read(&game).unwrap()],
                    top: ShortName::parse("GAME"),
                    format: FloppyFormat::Dd360,
                    label: volume_label("GAME").ok(),
                    split: true,
                    journal: true,
                    keep: None,
                    title: "GAME".into(),
                }),
                Place::Machine(id.clone()),
            ));
        shell.image_library.pump_until(|view| {
            view.machine_rows()
                .iter()
                .any(|row| row.id() == id && !row.created.is_empty())
        });
        let ctx = egui::Context::default();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "For A: at the next Start, in order · 2 disks",
                "GAME 1 of 2",
                "at Start",
                "GAME 2 of 2",
                crate::catalogue_menu::OWN_DISKS_AT_START
            ]
        );
        let chips = |shell: &mut Shell| -> Vec<String> {
            shell
                .drive_views()
                .iter()
                .map(crate::drive_chips::DriveView::chip_text)
                .collect()
        };
        assert_eq!(
            chips(&mut shell),
            ["A: GAME 1 of 2 (all 2 disks join the catalogue) · 360K · journal · at Start"]
        );
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            chips(&mut shell),
            ["A: GAME 2 of 2 (all 2 disks join the catalogue) · 360K · journal · at Start"]
        );
        let workspace = f.store.workspace_path(&id).unwrap();
        shell.image_library.pump_until(|_| {
            crate::created_media::read_pending(&workspace)
                .is_ok_and(|inserts| inserts.first().is_some_and(|i| i.first == Some(1)))
        });
    }

    /// An image's own disks are known only at Start too: with nothing
    /// waiting the item says so; the disks one Insert put in for Start are
    /// listed, and choosing another makes it the one that goes in.
    #[test]
    fn an_image_lists_the_disks_waiting_for_its_start() {
        let temp = machine_file::test_support::TempDir::new("shell-catalogue-image");
        let mut shell = shell();
        shell.stop();
        // An image of the library is selected, and no machine of it.
        let image =
            crate::library_view::machines::tests::row("dos", "DOS", "a", &["games:1"]).image_row();
        let config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
        shell.pending_media = Some(crate::media_dialog::PendingMedia::new(
            crate::media_dialog::PendingOwner::Image(image.descriptor.digest.clone()),
            config,
        ));
        shell.image_library.selected = Some(image);
        let name = shell.stopped_name();
        let catalogue = shell.catalogue(0, [None, None]).unwrap();
        assert_eq!(
            catalogue.disabled().map(|(label, _)| label).as_deref(),
            Some("Catalogue · known at Start")
        );
        let entries: Vec<_> = ["a.img", "b.img"]
            .into_iter()
            .map(|name| {
                let path = temp.path().join(name);
                std::fs::write(&path, vec![0; 1_474_560]).unwrap();
                crate::media_dialog::browse(&path, assembly::FloppyGeometry::HD_1_44).unwrap()
            })
            .collect();
        shell.pending_media.as_mut().unwrap().insert(0, entries, 0);
        let ctx = egui::Context::default();
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text),
            [
                "For A: at the next Start, in order · 2 disks",
                "a.img",
                "at Start",
                "b.img",
                crate::catalogue_menu::OWN_DISKS_AT_START
            ]
        );
        shell.command(Command::CatalogueDisk(0, 1), &ctx);
        assert_eq!(
            shell.message,
            format!("b.img goes into A: of {name} at its next Start.")
        );
        let chips: Vec<String> = shell
            .drive_views()
            .iter()
            .map(crate::drive_chips::DriveView::chip_text)
            .collect();
        assert_eq!(chips, ["A: b.img · 1.44M · read-only · at Start"]);
    }

    /// After Start, the disks a library machine took from a set keep the
    /// short names their set gives them: they lead the drive's catalogue,
    /// the chosen disk first, and the chip, This machine and the submenu
    /// name them as the set does.
    #[test]
    fn set_disks_put_in_at_start_keep_their_short_names() {
        let f = crate::library_test_support::Fixture::new();
        let set = import_win_set(&f);
        let mut shell = media_shell();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell
            .image_library
            .pump_until(|view| view.picker_set(&set.digest).is_some());
        let ids: Vec<String> = shell
            .image_library
            .picker_set(&set.digest)
            .unwrap()
            .disks
            .iter()
            .map(|disk| disk.id.clone())
            .collect();
        let insert = image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: set.clone(),
            reference: "win:3.1".into(),
            medium: ids[1].clone(),
            all: true,
            journal: false,
        };
        // The drive as the Start put the set in: disk 2, then 1 and 3, then
        // its own disk.
        let labels: Vec<String> = shell
            .image_library
            .inserted_sources(&insert)
            .into_iter()
            .map(|(_, label, _, _)| label)
            .collect();
        assert_eq!(labels, ["Disk 2", "Disk 1 (Setup)", "Disk 3"]);
        let mut config = assembly::bios(vec![0xF4; 32768], Some(vec![0; 1_474_560]));
        config.drives[0].media = Some(assembly::FloppyMediaList {
            empty: false,
            entries: labels
                .iter()
                .map(String::as_str)
                .chain(["own.img"])
                .map(|name| assembly::FloppyMedium {
                    image: assembly::ImageSource::Inline(vec![0; 1_474_560]),
                    expected_sha256: None,
                    write: assembly::WriteMode::ReadOnly,
                    name: name.into(),
                })
                .collect(),
            inserted: 0,
        });
        shell.presenter = Some(
            Presenter::new(
                &config,
                assembly::AssemblyOptions {
                    allow_unlisted_firmware: true,
                },
                &mut io::sink(),
            )
            .unwrap(),
        );
        shell.name_inserted_disks(&[insert]);
        let ctx = egui::Context::default();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(
            shows(&pass.text, "A: Win 3.1 · 2 of 3 · 1.44M · read-only"),
            "{:?}",
            pass.text
        );
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert_eq!(
            submenu_texts(&drawn.text)[1..],
            [
                "●",
                "Win 3.1 · 2 of 3",
                "in A:",
                "Win 3.1 · 1 of 3 — Setup",
                "Win 3.1 · 3 of 3",
                "own.img"
            ]
        );
        egui::Popup::close_all(&ctx);
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.take().unwrap();
        let names: Vec<&str> = picker.machine().drives[0]
            .catalogue
            .iter()
            .map(|disk| disk.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Win 3.1 · 2 of 3",
                "Win 3.1 · 1 of 3 — Setup",
                "Win 3.1 · 3 of 3",
                "own.img"
            ]
        );
        let tooltip = shell.drive_views()[0].tooltip(false);
        assert!(tooltip.contains(&(
            "Source",
            "Media set win:3.1 from the library · disk 2 of 3".into()
        )));
        assert!(shell.stop());
    }

    /// Tab to a drive chip and Enter open its menu; the keyboard reaches its
    /// Catalogue as it does the Media menu's.
    #[test]
    fn the_keyboard_reaches_the_catalogue_through_a_drive_chip() {
        use egui::Key::{ArrowDown, ArrowRight, Enter, Tab};
        let ctx = egui::Context::default();
        let mut shell = many_disk_shell(3);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .assembled()
            .change_floppy(
                assembly::DeviceId::Fdc,
                0,
                assembly::FloppyAction::Insert(1),
            )
            .unwrap();
        let press = |shell: &mut Shell, pressed: egui::Key| {
            let mut commands = status_pass(&ctx, shell, 1600.0, vec![key(pressed)]).commands;
            commands.extend(status_pass(&ctx, shell, 1600.0, Vec::new()).commands);
            let pass = status_pass(&ctx, shell, 1600.0, Vec::new());
            StatusPass {
                commands: [commands, pass.commands].concat(),
                ..pass
            }
        };
        status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let pass = press(&mut shell, Tab);
        assert!(
            pass.focused
                .as_deref()
                .is_some_and(|label| label.starts_with("Drive A: AZURAMON 2 of 3")),
            "{:?}",
            pass.focused
        );
        let mut pass = press(&mut shell, Enter);
        assert!(shows(&pass.text, "Create from files…"), "the chip's menu");
        for _ in 0..6 {
            if pass.focused.as_deref() == Some("Catalogue") {
                break;
            }
            pass = press(&mut shell, ArrowDown);
        }
        assert_eq!(pass.focused.as_deref(), Some("Catalogue"));
        let pass = press(&mut shell, ArrowRight);
        assert_eq!(pass.focused.as_deref(), Some("AZURAMON 2 of 3"));
        let pass = press(&mut shell, ArrowDown);
        assert_eq!(pass.focused.as_deref(), Some("AZURAMON 3 of 3"));
        let pass = press(&mut shell, Enter);
        assert_eq!(pass.commands, [Command::CatalogueDisk(0, 2)]);
        assert!(shell.stop());
    }

    /// Growing upward, the chip's submenu sits level with its item also in
    /// the frame its catalogue changes: it is measured again at once.
    #[test]
    fn the_chip_catalogue_is_measured_again_when_it_changes() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let frames = chip_catalogue(&ctx, &mut shell, 1040.0, 4);
        let item = label_rect(&frames[3].text, "Catalogue");
        let slot = shell.presenter.as_mut().unwrap().media_slots[0]
            .as_mut()
            .unwrap();
        let mut third = slot.entries[1].clone();
        third.index = Some(2);
        third.medium.name = "third.img".into();
        slot.entries.push(third);
        let pass = status_pass(
            &ctx,
            &mut shell,
            1040.0,
            vec![egui::Event::PointerMoved(item.center())],
        );
        let last = label_rect(&pass.text, "third.img");
        assert!(
            (last.bottom() - item.bottom()).abs() < 8.0,
            "{last:?} beside {item:?}"
        );
        assert!(shell.stop());
    }

    /// A library disk has one short name wherever its drive names it: the
    /// drive chip, the Media menu's drive line, This machine in the picker
    /// and the Catalogue submenu.
    #[test]
    fn a_library_disk_has_one_short_name_in_all_four_places() {
        let f = crate::library_test_support::Fixture::new();
        let set = f.import_set();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell
            .image_library
            .pump_until(|view| view.picker_set(&set.digest).is_some());
        let entry = shell.image_library.picker_set(&set.digest).unwrap();
        let short = crate::catalogue_menu::short_name(
            crate::catalogue_menu::set_disk(&entry, &entry.disks[0].id),
            "first.img",
        );
        assert_eq!(short, entry.title, "a one-disk set is its title");
        assert_ne!(short, "first.img");
        let slot = shell.presenter.as_mut().unwrap().media_slots[0]
            .as_mut()
            .unwrap();
        slot.entries[0].source = Some(image_store::media_set::CatalogueSource {
            set: set.digest.clone(),
            medium_id: entry.disks[0].id.clone(),
            blob: image_store::Digest::of(b"disk"),
        });
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(
            &pass.text,
            &format!("A: {short} · 1.44M · read-only")
        ));
        let drawn = media_catalogue(&ctx, &mut shell, 0);
        assert!(shows(
            &drawn.text,
            &format!("Drive A: (1.44M) — {short} · read-only")
        ));
        assert_eq!(
            submenu_texts(&drawn.text)[1..],
            ["●", short.as_str(), "in A:", "second.img"]
        );
        egui::Popup::close_all(&ctx);
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.as_ref().unwrap();
        assert_eq!(picker.machine().drives[0].catalogue[0].name, short);
        shell.picker = None;
        assert!(shell.stop());
    }

    /// Add files… and Add folder… answer on a later frame; the answer joins
    /// the Create view's list even after ‹ Back to search, which keeps it.
    #[test]
    fn a_dialog_answer_after_back_to_search_joins_the_kept_list() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.command(Command::CreateFromFiles(0), &ctx);
        let picker = shell.picker.as_mut().unwrap();
        assert_eq!(picker.mode, crate::media_picker::Mode::Create);
        picker.back_to_search();
        let path = std::path::PathBuf::from("/nonexistent/game");
        shell.file_chosen(FileFor::CreateFolder, Some(vec![path.clone()]));
        let picker = shell.picker.as_ref().unwrap();
        assert_eq!(picker.create.items.len(), 1);
        assert_eq!(picker.create.items[0].path, path);
        shell.picker = None;
        assert!(shell.stop());
    }

    /// Create and insert into a running machine: the disks are its own,
    /// stored by SHA-256 in its media folder; disk 1 goes in and every disk
    /// joins the drive's catalogue in order, so Next disk swaps them. Disks
    /// made for a machine that stopped meanwhile go into no other one.
    #[test]
    fn created_disks_go_into_the_running_machine_in_order() {
        use crate::created_media::{CreateOrder, Place};
        use media::fat12::{volume_label, FloppyFormat, HostItem, ShortName};
        let temp = machine_file::test_support::TempDir::new("shell-create-running");
        let root = temp.path().canonicalize().unwrap();
        let game = root.join("galaxia");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("a.dat"), vec![1; 900_000]).unwrap();
        std::fs::write(game.join("b.dat"), vec![2; 900_000]).unwrap();
        let store = image_store::Store::open(root.join("store")).unwrap();
        let order = CreateOrder {
            unit: 0,
            items: vec![HostItem::read(&game).unwrap()],
            top: ShortName::parse("GALAXIA"),
            format: FloppyFormat::Hd144,
            label: volume_label("GALAXIA").ok(),
            split: true,
            journal: true,
            keep: None,
            title: "GALAXIA".into(),
        };
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.args.machine = root.join("machine.toml");
        let media = shell.library_media_dir().unwrap();
        assert_eq!(
            media,
            image_store::location::machine_media_dir(&root.join("machine.toml")).unwrap()
        );
        let created =
            crate::created_media::create(&store, order.clone(), Place::Folder(media.clone()))
                .unwrap();
        assert_eq!(created.disks.len(), 2);
        shell.creating = shell.drive_owner();
        // The worker hashed the images; the window does not read them again.
        let hashed = journal_files::data_passes(1_474_560);
        shell.disks_created(Ok(Box::new(created.clone())));
        assert_eq!(journal_files::data_passes(1_474_560), hashed);
        assert!(shell.dialog.is_none(), "{:?}", shell.dialog);
        let name = shell.session_name().unwrap_or("the machine").to_owned();
        assert_eq!(
            shell.message,
            format!("GALAXIA (2 disks) is in A: of {name} now. Ctrl+Alt+F swaps to the next disk.")
        );
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(2)
            )]
        );
        let names: Vec<String> = presenter.media_slots[0]
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.medium.name.clone())
            .collect();
        assert_eq!(
            names,
            [
                "first.img",
                "second.img",
                "GALAXIA 1 of 2",
                "GALAXIA 2 of 2"
            ]
        );
        let name = |shell: &mut Shell| {
            shell
                .presenter
                .as_mut()
                .unwrap()
                .assembled()
                .floppy_name(assembly::DeviceId::Fdc, 0)
        };
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        assert_eq!(name(&mut shell), "GALAXIA 1 of 2");
        shell.command(Command::Next(0), &ctx);
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        assert_eq!(name(&mut shell), "GALAXIA 2 of 2");
        assert_eq!(
            std::fs::read_dir(media.join("created")).unwrap().count(),
            2,
            "each image once, by its SHA-256"
        );
        // Made for a drive the machine does not have: said, not left waiting.
        let mut other = created.clone();
        other.order.unit = 1;
        shell.creating = shell.drive_owner();
        shell.disks_created(Ok(Box::new(other)));
        assert!(shell.message.contains("B: is gone"), "{}", shell.message);
        // Made for a session that ended meanwhile: inserted nowhere.
        shell.creating = Some(DriveOwner::Session(shell.session + 1));
        shell.disks_created(Ok(Box::new(created)));
        assert!(
            shell.message.contains("was not inserted"),
            "{}",
            shell.message
        );
        assert_eq!(
            shell.presenter.as_ref().unwrap().media_slots[0]
                .as_ref()
                .unwrap()
                .entries
                .len(),
            4
        );
        assert!(shell.stop());
    }

    /// The picker a drive chip's Insert… opens keeps one size frame after
    /// frame, from This library and from This machine, so its rows stay
    /// where the pointer is and Insert stays on screen.
    #[test]
    fn the_picker_from_a_drive_chip_keeps_its_size() {
        let temp = machine_file::test_support::TempDir::new("chip-picker-size");
        let root = temp.path().canonicalize().unwrap();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        // A store path that does not exist: the user's own store is never read.
        shell.preferences.store_root = Some(root.join("no-store"));
        shell.args.machine = root.join("machine.toml");
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: first.img · 1.44M · read-only").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let insert = label_rect(&menu.text, "Insert…").center();
        let commands = click_status(&ctx, &mut shell, 1600.0, insert);
        assert_eq!(commands, [Command::Insert(0)]);
        shell.command(Command::Insert(0), &ctx);
        for source in [
            crate::media_picker::Source::Library,
            crate::media_picker::Source::Machine,
        ] {
            shell.picker.as_mut().unwrap().set_source(source);
            crate::library_test_support::settled_modal(
                egui::Id::unique("media-picker"),
                Vec2::new(1040.0, 700.0),
                |ctx| shell.picker_modal(ctx),
            );
            assert!(shell.picker.is_some(), "the picker stays open");
        }
        shell.picker = None;
        assert!(shell.stop());
    }

    #[test]
    fn a_set_chosen_in_the_picker_loads_into_the_running_drive_in_order() {
        let temp = machine_file::test_support::TempDir::new("library-set-insert");
        let root = temp.path().canonicalize().unwrap();
        let store = root.join("store");
        for (name, byte) in [("one.img", 1u8), ("two.img", 2)] {
            std::fs::write(root.join(name), vec![byte; 1_474_560]).unwrap();
        }
        let (one, two) = (root.join("one.img"), root.join("two.img"));
        crate::library_test_support::import_media_set(
            &image_store::Store::open(&store).unwrap(),
            "test-set:r1",
            "Synthetic install",
            &[
                crate::library_test_support::Medium::new(&one).label("Setup"),
                crate::library_test_support::Medium::new(&two).label("Data"),
            ],
            &[],
        );
        let cache = crate::library_view::install_media::tests::warm(
            &image_store::Store::open_existing(&store).unwrap(),
        );
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.args.machine = root.join("machine.toml");
        // The set lives in the library's store, whatever the preference says.
        shell.preferences.store_root = Some(root.join("no-store"));
        shell.image_library = crate::library_view::LibraryView::new(Some(store.clone()));
        assert!(
            !shell.command_allowed(Command::InsertSet(0)),
            "no set is selected"
        );
        shell
            .image_library
            .select_set(&cache, "test-set:r1", &mut shell.selected);
        assert!(shell.command_allowed(Command::InsertSet(0)));
        assert!(
            !shell.command_allowed(Command::InsertSet(1)),
            "the machine has no drive B:"
        );
        // The picker lists the library's sets, with the selected one chosen.
        shell.image_library.use_snapshot(&cache);
        shell.command(Command::InsertSet(0), &ctx);
        let picker = shell
            .picker
            .as_ref()
            .expect("Insert of a set opens the media picker");
        let Some(crate::media_picker::Choice::Set(chosen)) = picker.choice else {
            panic!("the selected set is chosen: {:?}", picker.choice);
        };
        assert_eq!(picker.sets[chosen].reference, "test-set:r1");
        let outcome = picker.insert().expect("the chosen set goes in");
        assert!(matches!(
            &outcome,
            crate::media_picker::Outcome::Set { unit: 0, reference, first: 0, all: true, journal: true, .. }
                if reference == "test-set:r1"
        ));
        shell.apply_picker(outcome);
        let draft = shell
            .insert_dialog
            .as_mut()
            .expect("the drive's insert, without its dialog");
        assert_eq!(draft.unit, 0);
        assert!(draft.automatic);
        let library = draft.library.as_mut().expect("the set's library insert");
        assert_eq!(library.store_path, store.to_string_lossy());
        for _ in crate::test_wait::rounds() {
            assert_eq!(library.poll(), None);
            if !library.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let chosen = library.rows.iter().position(|row| row.tag == "test-set:r1");
        assert!(chosen.is_some());
        assert_eq!(
            library.selected, chosen,
            "the library window's set is chosen"
        );
        // The frames themselves finish the insert, without its dialog.
        modal_frames(&ctx, &mut shell, |shell| shell.insert_dialog.is_none());
        assert_eq!(shell.dialog, None);
        let presenter = shell.presenter.as_ref().unwrap();
        let names: Vec<_> = presenter.media_slots[0]
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.medium.name.clone())
            .collect();
        assert_eq!(names, ["first.img", "second.img", "Setup", "Data"]);
        assert_eq!(
            presenter.media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(2)
            )]
        );
        assert!(shell.stop());
    }

    #[test]
    fn drive_chip_hover_shows_name_source_hash_and_write_mode() {
        let ctx = egui::Context::default();
        ctx.global_style_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let mut shell = media_shell();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: first.img · 1.44M · read-only").center();
        let mut hover = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        for _ in 0..3 {
            hover = status_pass(
                &ctx,
                &mut shell,
                1600.0,
                vec![egui::Event::PointerMoved(chip)],
            );
        }
        for wanted in [
            "A: 3.5\" 1.44M",
            "first.img",
            "Machine file at-freedos · disk 1 of 2",
            "not pinned",
            "1.44M · 80/2/18",
            "Read-only — the guest's writes are refused; fixed by the machine file",
            "Click for this drive's menu",
        ] {
            assert!(shows(&hover.text, wanted), "{wanted} missing");
        }
        assert!(!shows(&hover.text, "Activity"));
    }

    #[test]
    fn drive_chip_click_opens_that_drives_menu_with_the_media_commands() {
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(!shows(&pass.text, "Eject A:"));
        let chip = label_rect(&pass.text, "A: first.img · 1.44M · read-only").center();
        assert!(click_status(&ctx, &mut shell, 1600.0, chip).is_empty());
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        for item in [
            "Drive A: (1.44M) — first.img · read-only",
            "Eject A:",
            "Insert…",
            "Write mode: Read-only, fixed by the machine file",
        ] {
            assert!(shows(&menu.text, item), "{item} missing");
        }
        assert!(!shows(&menu.text, "Change write mode…"));
        let eject = label_rect(&menu.text, "Eject A:").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, eject),
            [Command::Eject(0)]
        );
        let closed = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(!shows(&closed.text, "Insert…"));
        assert!(!shows(&menu.text, "From library…"));
        shell.command(Command::Eject(0), &ctx);
        assert_eq!(
            shell.presenter.as_ref().unwrap().media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Eject
            )]
        );

        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let insert = label_rect(&menu.text, "Insert…").center();
        let commands = click_status(&ctx, &mut shell, 1600.0, insert);
        assert_eq!(commands, [Command::Insert(0)]);
        shell.command(Command::Insert(0), &ctx);
        assert_eq!(shell.picker.as_ref().unwrap().unit, 0);
        assert!(shell.stop());
    }

    #[test]
    fn fixed_disk_chip_menu_offers_disk_layers_and_cannot_eject() {
        let ctx = egui::Context::default();
        let mut shell = busy_shell();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "C: inline image · read-only").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        for item in [
            "C: IDE master — inline image · read-only",
            "Eject (fixed disk, part of the machine)",
            "Disk layers…",
            "Write mode: Read-only, fixed by the machine file",
        ] {
            assert!(shows(&menu.text, item), "{item} missing");
        }
        assert!(!shows(&menu.text, "From library…"));
        let layers = label_rect(&menu.text, "Disk layers…").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, layers),
            [Command::Layers]
        );
        assert!(shell.stop());
    }

    #[test]
    fn session_floppy_write_mode_changes_through_its_insert_dialog() {
        let temp = machine_file::test_support::TempDir::new("chip-write-mode");
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("extra.img");
        std::fs::write(&path, vec![0; 1_474_560]).unwrap();
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.command(Command::InsertFile(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        draft.add_browsed(crate::media_dialog::browse(&path, draft.slot.geometry).unwrap());
        shell.confirm_insert();
        shell
            .presenter
            .as_mut()
            .unwrap()
            .apply_media_events()
            .unwrap();

        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        let chip = label_rect(&pass.text, "A: extra.img · 1.44M · read-only").center();
        click_status(&ctx, &mut shell, 1600.0, chip);
        let menu = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(&menu.text, "Write mode: Read-only"));
        assert!(!shows(&menu.text, "fixed by the machine file"));
        let change = label_rect(&menu.text, "Change write mode…").center();
        assert_eq!(
            click_status(&ctx, &mut shell, 1600.0, change),
            [Command::WriteMode(0)]
        );
        shell.command(Command::WriteMode(0), &ctx);
        let draft = shell.insert_dialog.as_mut().unwrap();
        assert_eq!(draft.selected, Some(2), "the inserted image is preselected");
        assert!(!draft.journal);
        draft.journal = true;
        draft.journal_path = "extra.upfj".into();
        shell.confirm_insert();
        shell
            .presenter
            .as_mut()
            .unwrap()
            .apply_media_events()
            .unwrap();
        let pass = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert!(shows(&pass.text, "A: extra.img · 1.44M · journal"));
        shell.command(Command::WriteMode(0), &ctx);
        let draft = shell.insert_dialog.as_ref().unwrap();
        assert_eq!(draft.selected, Some(2));
        assert!(draft.journal, "the dialog shows the current mode");
        shell.insert_dialog = None;
        assert!(shell.stop());
    }

    #[test]
    fn media_menu_names_the_image_and_write_mode_in_each_drive() {
        let ctx = egui::Context::default();
        let open_media = |shell: &mut Shell| {
            let drawn = draw(&ctx, shell, Vec::new());
            click(&ctx, shell, label_rect(&drawn.text, "Media").center());
            let drawn = draw(&ctx, shell, Vec::new());
            egui::Popup::close_all(&ctx);
            draw(&ctx, shell, Vec::new());
            drawn.text
        };
        let mut shell = media_shell();
        let text = open_media(&mut shell);
        for label in [
            "Drive A: (1.44M) — first.img · read-only",
            "Drive B: — not installed",
            "C: — no fixed disk",
        ] {
            assert!(shows(&text, label), "{label} missing");
        }
        shell.command(Command::Next(0), &ctx);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .apply_media_events()
            .unwrap();
        assert!(shows(
            &open_media(&mut shell),
            "Drive A: (1.44M) — second.img · read-only"
        ));
        let mut shell = busy_shell();
        assert!(shows(
            &open_media(&mut shell),
            "C: IDE master — inline image · read-only"
        ));
        assert!(shell.stop());
        let text = open_media(&mut shell);
        assert!(shows(&text, "Drive A: — machine stopped"));
        assert!(shows(&text, "C: — machine stopped"));
    }

    #[test]
    fn chip_light_follows_the_sampled_busy_state_with_a_host_time_hold() {
        let ctx = egui::Context::default();
        let mut shell = busy_shell();
        let leds = |pass: &StatusPass| {
            pass.fills
                .iter()
                .filter(|(fill, rect)| *fill == AMBER && rect.width() == 10.0)
                .count()
        };
        let before = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert_eq!(leds(&before), 0, "no boundary has been sampled yet");
        shell.presenter.as_mut().unwrap().full_slices = true;
        shell.presenter.as_mut().unwrap().next_paint = Some(std::time::Duration::ZERO);
        shell.tick(&ctx, false, std::time::Duration::from_millis(20));
        // The presenter has no start instant here, so the shell's host time is zero.
        let lit = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert_eq!(leds(&lit), 2, "A: motor and C: DRQ");
        assert!(lit
            .labels
            .iter()
            .any(|label| label == "Drive A: disk 1 · 1.44M · volatile, active"));
        assert!(lit
            .labels
            .iter()
            .any(|label| label == "Drive C: inline image · read-only, active"));
        shell.presenter.as_mut().unwrap().started =
            Some(Instant::now() - std::time::Duration::from_secs(5));
        let held_out = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
        assert_eq!(leds(&held_out), 0, "the hold has ended without new samples");
        assert!(held_out
            .labels
            .iter()
            .any(|label| label == "Drive A: disk 1 · 1.44M · volatile"));
        assert!(shell.stop());
    }

    struct Drawn {
        text: Vec<(String, egui::Rect)>,
        controls: Vec<(String, bool)>,
        /// Selectable items and whether each is marked as the current choice.
        toggled: Vec<(String, bool)>,
        commands: Vec<Command>,
        /// The label of the widget with the keyboard focus.
        focused: Option<String>,
    }

    /// Draw the menu bar above the toolbar, as the shell's chrome panel does.
    fn draw(ctx: &egui::Context, shell: &mut Shell, events: Vec<egui::Event>) -> Drawn {
        ctx.enable_accesskit();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1040.0, 700.0),
            )),
            events,
            ..Default::default()
        };
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(input, |ui| {
            shell.menus(ui, &mut commands);
            shell.toolbar(ui, &mut commands);
        });
        output.textures_delta.clear();
        let text = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some((
                    text.galley.text().to_owned(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                _ => None,
            })
            .collect();
        let update = output.platform_output.accesskit_update.take();
        let controls = update
            .as_ref()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| Some((node.label()?.to_owned(), node.is_disabled())))
                    .collect()
            })
            .unwrap_or_default();
        let toggled = update
            .as_ref()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| {
                        Some((
                            node.label()?.to_owned(),
                            node.toggled()? == egui::accesskit::Toggled::True,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let focused = ctx.memory(|memory| memory.focused()).and_then(|id| {
            let update = update.as_ref()?;
            update
                .nodes
                .iter()
                .find(|(node, _)| *node == id.accesskit_id())
                .and_then(|(_, node)| node.label().map(str::to_owned))
        });
        output.drop_without_applying_deltas();
        Drawn {
            text,
            controls,
            toggled,
            commands,
            focused,
        }
    }

    fn click(ctx: &egui::Context, shell: &mut Shell, at: egui::Pos2) -> Vec<Command> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let mut commands = draw(
            ctx,
            shell,
            vec![egui::Event::PointerMoved(at), button(true)],
        )
        .commands;
        commands.extend(draw(ctx, shell, vec![button(false)]).commands);
        commands
    }

    fn toolbar_pass(
        ctx: &egui::Context,
        shell: &mut Shell,
        events: Vec<egui::Event>,
    ) -> (Vec<(String, egui::Rect)>, Vec<Command>) {
        let drawn = draw(ctx, shell, events);
        (drawn.text, drawn.commands)
    }

    fn click_toolbar(ctx: &egui::Context, shell: &mut Shell, at: egui::Pos2) -> Vec<Command> {
        click(ctx, shell, at)
    }

    /// The speed items currently marked as chosen (menus also mark Toolbar,
    /// Status bar and the scale mode).
    fn marked(drawn: &Drawn) -> Vec<&str> {
        drawn
            .toggled
            .iter()
            .filter(|(label, on)| {
                *on && crate::pacing::Speed::ALL
                    .iter()
                    .any(|speed| speed.label() == label)
            })
            .map(|(label, _)| label.as_str())
            .collect()
    }

    fn label_rect(text: &[(String, egui::Rect)], label: &str) -> egui::Rect {
        text.iter()
            .find(|(shown, _)| shown == label)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{label} not shown in {text:?}"))
    }

    /// Open View › Speed, return what the submenu draws, then close the menus.
    fn speed_menu(ctx: &egui::Context, shell: &mut Shell) -> Drawn {
        let mut drawn = draw(ctx, shell, Vec::new());
        click(ctx, shell, label_rect(&drawn.text, "View").center());
        drawn = draw(ctx, shell, Vec::new());
        let speed = label_rect(&drawn.text, "Speed").center();
        click(ctx, shell, speed);
        for _ in 0..2 {
            drawn = draw(ctx, shell, vec![egui::Event::PointerMoved(speed)]);
        }
        egui::Popup::close_all(ctx);
        draw(ctx, shell, Vec::new());
        drawn
    }

    #[test]
    fn toolbar_speed_selector_changes_pacing_and_the_menu_check() {
        use crate::pacing::{virtual_ticks, Speed};
        use std::time::{Duration, Instant};
        let mut shell = shell();
        let ctx = egui::Context::default();
        // Ten seconds into the session, with the guest behind the host clock.
        let presenter = shell.presenter.as_mut().unwrap();
        presenter.started = Instant::now().checked_sub(Duration::from_secs(10));
        presenter.advance_fully(Duration::from_secs(10)).unwrap();
        assert!(guest_clock(&shell) > 0);
        let mut text = Vec::new();
        for _ in 0..2 {
            text = toolbar_pass(&ctx, &mut shell, Vec::new()).0;
        }
        let layers = label_rect(&text, "Layers");
        let label = label_rect(&text, "Speed:");
        let selected = label_rect(&text, "100%");
        assert!(layers.right() < label.left() && label.right() < selected.left());
        assert!((label.center().y - layers.center().y).abs() < 4.0);
        assert!(!shows(&text, "200%"));
        assert_eq!(marked(&speed_menu(&ctx, &mut shell)), ["100%"]);

        assert!(click_toolbar(&ctx, &mut shell, selected.center()).is_empty());
        let mut drawn = draw(&ctx, &mut shell, Vec::new());
        for _ in 0..2 {
            drawn = draw(&ctx, &mut shell, Vec::new());
        }
        for speed in Speed::ALL {
            assert!(
                shows(&drawn.text, speed.label()),
                "{speed:?} missing from the list"
            );
        }
        assert_eq!(marked(&drawn), ["100%"]);
        let commands = click_toolbar(&ctx, &mut shell, label_rect(&drawn.text, "200%").center());
        assert_eq!(commands, [Command::Speed(Speed::Double)]);
        let clock = guest_clock(&shell);
        let before = shell.elapsed();
        for command in commands {
            shell.command(command, &ctx);
        }
        let after = shell.elapsed();
        assert!(before >= Duration::from_secs(10));
        assert_eq!(shell.selected_speed(), Speed::Double);
        assert_eq!(marked(&speed_menu(&ctx, &mut shell)), ["200%"]);

        // Re-anchored at the change: the 200% target owes only the moments since
        // then, not the full 100 ms frame allowance a stale anchor would grant.
        let slack = Duration::from_millis(1);
        let presenter = shell.presenter.as_mut().unwrap();
        let owed = presenter.pacing.target(after, clock).unwrap() - clock;
        assert!(u128::from(owed) <= virtual_ticks((after - before + slack) * 2));
        assert!(u128::from(owed) < virtual_ticks(Duration::from_millis(100)));
        // No catch-up burst on the next paint: 10 ms of host time runs about
        // 20 ms of guest time at 200%.
        presenter
            .advance_fully(after + Duration::from_millis(10))
            .unwrap();
        let ran = u128::from(guest_clock(&shell) - clock);
        assert!(ran >= virtual_ticks(Duration::from_millis(19)), "{ran}");
        assert!(
            ran <= virtual_ticks((after - before + Duration::from_millis(10) + slack) * 2),
            "{ran}"
        );
        let text = toolbar_pass(&ctx, &mut shell, Vec::new()).0;
        assert!(shows(&text, "200%"));
        assert!(!shows(&text, "100%"));
    }

    #[test]
    fn toolbar_list_marks_the_persistent_speed_during_a_hold_and_clicks_set_it() {
        use crate::pacing::Speed;
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.command(Command::Speed(Speed::Quadruple), &ctx);
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyT] {
            shell.key(key, Pressed, false, &ctx);
        }
        assert_eq!(shell.toolbar_speed(), Speed::Unlimited);
        let mut text = Vec::new();
        for _ in 0..2 {
            text = toolbar_pass(&ctx, &mut shell, Vec::new()).0;
        }
        assert!(
            click_toolbar(&ctx, &mut shell, label_rect(&text, "Unlimited").center()).is_empty()
        );
        let mut drawn = draw(&ctx, &mut shell, Vec::new());
        for _ in 0..2 {
            drawn = draw(&ctx, &mut shell, Vec::new());
        }
        // The closed box reads Unlimited; the list marks the persistent 400%.
        assert_eq!(marked(&drawn), ["400%"]);
        let commands = click_toolbar(&ctx, &mut shell, label_rect(&drawn.text, "200%").center());
        assert_eq!(commands, [Command::Speed(Speed::Double)]);
        for command in commands {
            shell.command(command, &ctx);
        }
        assert_eq!(shell.selected_speed(), Speed::Double);
        assert_eq!(shell.toolbar_speed(), Speed::Unlimited);
        shell.key(KeyCode::KeyT, Released, false, &ctx);
        assert_eq!(shell.toolbar_speed(), Speed::Double);
    }

    #[test]
    fn toolbar_speed_shows_the_held_fast_forward_and_restores_the_selection() {
        use crate::pacing::Speed;
        let mut shell = keyboard_shell();
        let ctx = egui::Context::default();
        shell.command(Command::Speed(Speed::Quadruple), &ctx);
        let shown = |shell: &mut Shell| {
            let text = toolbar_pass(&ctx, shell, Vec::new()).0;
            Speed::ALL
                .into_iter()
                .filter(|speed| shows(&text, speed.label()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&mut shell), [Speed::Quadruple]);
        for key in [KeyCode::ControlLeft, KeyCode::AltLeft, KeyCode::KeyT] {
            shell.key(key, Pressed, false, &ctx);
        }
        assert_eq!(shell.toolbar_speed(), Speed::Unlimited);
        assert_eq!(shown(&mut shell), [Speed::Unlimited]);
        assert_eq!(shell.selected_speed(), Speed::Quadruple);
        assert!(shell.speed_status().starts_with("Fast-forward"));
        shell.key(KeyCode::KeyT, Released, false, &ctx);
        assert_eq!(shown(&mut shell), [Speed::Quadruple]);
        assert_eq!(shell.speed_status(), "Speed 400%");
        for key in [KeyCode::AltLeft, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        assert!(shell.command_allowed(Command::Speed(Speed::Normal)));
        shell.stop();
        assert!(!shell.command_allowed(Command::Speed(Speed::Normal)));
        assert_eq!(shown(&mut shell), [Speed::Normal]);
    }

    fn command_q(shell: &mut Shell, blocked: bool, ctx: &egui::Context) {
        shell.key(KeyCode::SuperLeft, Pressed, blocked, ctx);
        shell.keyboard(
            HostKey {
                key: KeyCode::KeyQ,
                shortcut_key: Some(KeyCode::KeyQ),
                state: Pressed,
                repeat: false,
                synthetic: false,
                guest_focus: shell.guest_focus,
                session: shell.session,
            },
            blocked,
            ctx,
        );
        shell.key(KeyCode::KeyQ, Released, blocked, ctx);
        shell.key(KeyCode::SuperLeft, Released, blocked, ctx);
    }

    #[test]
    fn quitting_closes_an_open_dialog_or_menu_and_then_asks() {
        let temp = machine_file::test_support::TempDir::new("exit-through-dialogs");
        std::fs::create_dir(temp.path().join("machines")).unwrap();
        let machine = temp.path().join("machines/example.toml");
        std::fs::write(
            &machine,
            "schema=1\nextends='preset:ibm-5170'\n[firmware.bios]\nimage='missing.bin'\n",
        )
        .unwrap();
        for open in [
            "about", "insert", "blank", "settings", "layers", "menu", "library",
        ] {
            for close_button in [false, true] {
                let mut shell = media_shell();
                shell.macos_shortcuts = true;
                shell.library.push(LibraryEntry::pending(machine.clone()));
                shell.selected = Some(0);
                let ctx = egui::Context::default();
                match open {
                    "about" => shell.command(Command::About, &ctx),
                    "insert" => shell.command(Command::Insert(0), &ctx),
                    "blank" => shell.command(Command::CreateBlank, &ctx),
                    "settings" => shell.command(Command::Settings, &ctx),
                    "layers" => shell.command(Command::Layers, &ctx),
                    "menu" => egui::Popup::open_id(&ctx, egui::Id::unique("an open menu")),
                    _ => shell.command(Command::Library, &ctx),
                }
                let menu = egui::Popup::is_any_open(&ctx);
                assert!(shell.has_dialog() || menu, "{open}");
                let (commands, _) = if close_button {
                    let mut input = egui::RawInput::default();
                    input
                        .viewports
                        .entry(egui::ViewportId::ROOT)
                        .or_default()
                        .events
                        .push(egui::ViewportEvent::Close);
                    pass(&ctx, input, |ctx| shell.window_close(ctx))
                } else {
                    pass(&ctx, Default::default(), |ctx| {
                        command_q(&mut shell, menu, ctx)
                    })
                };
                assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit), "{open}");
                assert!(commands.contains(&egui::ViewportCommand::CancelClose));
                assert!(!egui::Popup::is_any_open(&ctx), "{open}");
                assert!(shell.dialog.is_none(), "{open}");
                assert!(shell.insert_dialog.is_none() && shell.blank_disk.is_none());
                assert!(shell.settings.is_none(), "{open}");
                assert!(!shell.image_library.has_dialog(), "{open}");
                assert!(shell.presenter.as_ref().unwrap().running);
                pass(&ctx, Default::default(), |ctx| {
                    shell.answer_stop_prompt(true, ctx)
                });
                assert!(shell.presenter.is_none());
            }
        }
        // A failed machine has nothing to ask about: its error dialog closes and
        // the window closes.
        let mut shell = media_shell();
        shell.macos_shortcuts = true;
        shell.show_error("cpu failure".into());
        let ctx = egui::Context::default();
        let (commands, _) = pass(&ctx, Default::default(), |ctx| {
            command_q(&mut shell, false, ctx)
        });
        assert!(commands.contains(&egui::ViewportCommand::Close));
        assert!(shell.dialog.is_none());
        assert_eq!(shell.stop_prompt, None);
    }

    #[test]
    fn closing_the_open_machine_dialog_leaves_the_session_running() {
        use std::time::Duration;
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.command(Command::Placeholder("Open machine file"), &ctx);
        assert_eq!(shell.stop_prompt, None);
        assert!(shell.presenter.as_ref().unwrap().occluded_since.is_none());
        shell.open_path = "never-opened.toml".into();
        // The dialog's Cancel button only drops the dialog.
        shell.dialog = None;
        shell.presenter.as_mut().unwrap().full_slices = true;
        shell.presenter.as_mut().unwrap().next_paint = Some(Duration::ZERO);
        shell.tick(&ctx, false, Duration::from_millis(20));
        assert!(guest_clock(&shell) > 0);
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(shell.library.is_empty());
        assert_eq!(shell.stop_prompt, None);
    }

    #[test]
    fn one_escape_closes_only_the_top_most_dialog() {
        let mut shell = media_shell();
        let ctx = egui::Context::default();
        shell.command(Command::Library, &ctx);
        shell.command(Command::InsertFile(0), &ctx);
        assert!(shell.stop_prompt.is_some() && shell.insert_dialog.is_some());
        let escape = || egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        };
        // The shell's pass order: the stop prompt takes Escape first, then the
        // dialogs beneath it draw, then the prompt itself.
        let frame = |shell: &mut Shell, input| {
            let mut answer = None;
            pass(&ctx, input, |ctx| {
                let stop_escape = shell.stop_prompt.is_some() && ctx.input_mut(escape_pressed);
                shell.media_modals(ctx);
                answer = shell.stop_prompt_modal(ctx, stop_escape);
            });
            if let Some(stop) = answer {
                shell.answer_stop_prompt(stop, &ctx);
            }
            answer
        };
        frame(&mut shell, Default::default());
        assert_eq!(frame(&mut shell, escape()), Some(false));
        assert!(shell.insert_dialog.is_some());
        assert!(shell.presenter.is_some());
        frame(&mut shell, escape());
        assert!(shell.insert_dialog.is_none());
    }

    #[test]
    fn enter_and_space_never_answer_the_stop_prompt() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.command(Command::Exit, &ctx);
        for key in [egui::Key::Enter, egui::Key::Space] {
            for pressed in [true, false, true, false] {
                let input = egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers: Default::default(),
                    }],
                    ..Default::default()
                };
                let mut answer = None;
                pass(&ctx, input, |ctx| {
                    answer = shell.stop_prompt_modal(ctx, false)
                });
                assert_eq!(answer, None, "{key:?}");
            }
        }
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(shell.presenter.as_ref().unwrap().running);
    }

    #[test]
    fn cancel_hands_guest_focus_and_pointer_capture_back() {
        let ctx = egui::Context::default();
        for answer_stop in [false, true] {
            let mut shell = captured_shell();
            shell.macos_shortcuts = true;
            assert!(shell.guest_focus);
            command_q(&mut shell, false, &ctx);
            assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
            assert!(!shell.guest_focus);
            assert!(!shell.presenter.as_ref().unwrap().mouse.captured());
            assert!(!shell.events.borrow().captured);
            shell.answer_stop_prompt(answer_stop, &ctx);
            // The pass that closed the prompt still releases input.
            shell.resume_input(true);
            assert!(!shell.guest_focus);
            shell.resume_input(false);
            if answer_stop {
                assert!(shell.presenter.is_none());
                assert!(!shell.guest_focus);
                continue;
            }
            assert!(shell.guest_focus);
            assert!(shell.presenter.as_ref().unwrap().mouse.captured());
            let events = shell.events.borrow();
            assert!(events.guest_focus && events.captured);
        }
        // A prompt opened from a menu had neither, and restores neither.
        let mut shell = captured_shell();
        shell.release_pointer();
        shell.release();
        shell.command(Command::Library, &ctx);
        shell.answer_stop_prompt(false, &ctx);
        shell.resume_input(false);
        assert!(!shell.guest_focus);
        assert!(!shell.presenter.as_ref().unwrap().mouse.captured());
    }

    #[test]
    fn modifier_changes_reach_egui() {
        let mut events = Events::default();
        assert!(events.forward(&WindowEvent::ModifiersChanged(ModifiersState::SUPER.into())));
        assert!(matches!(
            events.queue.last(),
            Some(HostEvent::Window(event)) if matches!(**event, WindowEvent::ModifiersChanged(_))
        ));
    }

    /// A request as the native application layer delivers it.
    fn native(shell: &Shell, request: crate::app_menu::NativeRequest) {
        shell.events.borrow_mut().native.push(request);
    }

    /// The replies AppKit gets, oldest first.
    fn replies(shell: &Shell) -> Rc<RefCell<Vec<bool>>> {
        let replies = Rc::new(RefCell::new(Vec::new()));
        let log = replies.clone();
        shell.events.borrow_mut().native.on_reply =
            Some(Box::new(move |terminate| log.borrow_mut().push(terminate)));
        replies
    }

    /// One shell pass as far as a system quit sees it.
    fn quit_pass(shell: &mut Shell, ctx: &egui::Context) -> Vec<egui::ViewportCommand> {
        pass(ctx, Default::default(), |ctx| {
            shell.input(ctx, false);
            shell.window_close(ctx);
        })
        .0
    }

    #[test]
    fn application_menu_about_and_quit_run_shell_commands_like_hotkeys() {
        use crate::app_menu::NativeRequest;
        let mut shell = shell();
        let replies = replies(&shell);
        let ctx = egui::Context::default();
        native(&shell, NativeRequest::Command(Command::About));
        // The menu works while the window is minimized; what it opens comes forward.
        shell.minimized = true;
        let commands = pass(&ctx, Default::default(), |ctx| shell.visibility(ctx, true)).0;
        assert_eq!(
            shell.dialog.as_ref().map(|(title, _)| *title),
            Some("About unemu")
        );
        assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        shell.minimized = false;
        // An open dialog holds back everything but quitting.
        native(&shell, NativeRequest::Command(Command::Shortcuts));
        let commands = quit_pass(&mut shell, &ctx);
        assert_eq!(
            shell.dialog.as_ref().map(|(title, _)| *title),
            Some("About unemu")
        );
        assert!(!commands.contains(&egui::ViewportCommand::Focus));
        // Quit closes the dialog and asks, exactly like File › Exit; a menu
        // Quit owes AppKit no reply.
        native(&shell, NativeRequest::Command(Command::Exit));
        let commands = quit_pass(&mut shell, &ctx);
        assert!(shell.dialog.is_none());
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        shell.answer_stop_prompt(true, &ctx);
        let commands = quit_pass(&mut shell, &ctx);
        assert!(shell.presenter.is_none());
        assert!(commands.contains(&egui::ViewportCommand::Close));
        assert!(replies.borrow().is_empty());
    }

    #[test]
    fn system_quit_without_a_running_machine_terminates_in_the_same_pass() {
        let mut shell = shell();
        assert!(shell.stop());
        let replies = replies(&shell);
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        let commands = quit_pass(&mut shell, &ctx);
        assert!(shell.exit_requested);
        assert_eq!(shell.stop_prompt, None);
        assert_eq!(*replies.borrow(), [true]);
        // AppKit closes the window and ends the process itself.
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        assert!(!commands.contains(&egui::ViewportCommand::Focus));
        assert!(quit_pass(&mut shell, &ctx).is_empty());
        assert_eq!(*replies.borrow(), [true]);
    }

    #[test]
    fn system_quit_while_running_asks_and_stop_terminates_after_persistence() {
        let temp = machine_file::test_support::TempDir::new("shell-system-quit-stop");
        let journal = temp.path().join("disk.upfj");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        // The journal's length when each reply is given.
        let replies = Rc::new(RefCell::new(Vec::new()));
        let (log, path) = (replies.clone(), journal.clone());
        shell.events.borrow_mut().native.on_reply = Some(Box::new(move |terminate| {
            let length = std::fs::metadata(&path).unwrap().len();
            log.borrow_mut().push((terminate, length));
        }));
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        let commands = quit_pass(&mut shell, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(shell.presenter.as_ref().unwrap().occluded_since.is_some());
        // The question comes forward, even out of a hidden or minimized window.
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        let commands = quit_pass(&mut shell, &ctx);
        assert!(!commands.contains(&egui::ViewportCommand::Focus));
        assert!(replies.borrow().is_empty());
        let open = std::fs::metadata(&journal).unwrap().len();
        let commands = pass(&ctx, Default::default(), |ctx| {
            shell.answer_stop_prompt(true, ctx);
            shell.input(ctx, false);
            shell.window_close(ctx);
        })
        .0;
        assert!(shell.presenter.is_none());
        let finished = std::fs::metadata(&journal).unwrap().len();
        assert!(finished > open);
        // The reply came only once the session's journal was finished.
        assert_eq!(*replies.borrow(), [(true, finished)]);
        assert!(shell.errors.borrow().is_none());
        assert!(!commands.contains(&egui::ViewportCommand::Close));
    }

    #[test]
    fn system_quit_cancel_keeps_running_and_the_next_quit_asks_again() {
        let mut shell = shell();
        let replies = replies(&shell);
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        quit_pass(&mut shell, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        // A Dock Quit while asking joins the open question.
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        quit_pass(&mut shell, &ctx);
        assert!(replies.borrow().is_empty());
        let commands = pass(&ctx, Default::default(), |ctx| {
            shell.answer_stop_prompt(false, ctx);
            shell.input(ctx, false);
            shell.window_close(ctx);
        })
        .0;
        assert_eq!(*replies.borrow(), [false]);
        assert_eq!(shell.stop_prompt, None);
        assert!(!shell.exit_requested);
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        let commands = quit_pass(&mut shell, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        assert_eq!(*replies.borrow(), [false]);
    }

    /// A stopped Gaming-mode session whose keep-or-discard question is open.
    fn gaming_decision(shell: &mut Shell) {
        assert!(shell.stop());
        shell.image_library.decision = Some(crate::library_worker::Recovery {
            schema: 1,
            kind: crate::library_worker::WorkspaceKind::Disposable,
            reference: "games:1".into(),
            image: image_store::Descriptor::new(
                image_store::media_type::CONFIG,
                image_store::Digest::of(b"gaming"),
                6,
            ),
            title: "Games".into(),
            recovery: true,
            machine: None,
            name: "gaming-workspace".into(),
        });
    }

    #[test]
    fn system_quit_waits_for_the_gaming_mode_decision_before_terminating() {
        let mut shell = shell();
        gaming_decision(&mut shell);
        let replies = replies(&shell);
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        let commands = quit_pass(&mut shell, &ctx);
        assert!(shell.exit_requested);
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        assert!(replies.borrow().is_empty());
        let commands = quit_pass(&mut shell, &ctx);
        assert!(!commands.contains(&egui::ViewportCommand::Focus));
        shell.image_library.decision = None;
        let commands = quit_pass(&mut shell, &ctx);
        assert_eq!(*replies.borrow(), [true]);
        assert!(!commands.contains(&egui::ViewportCommand::Close));
    }

    #[test]
    fn system_quit_brings_a_hidden_or_minimized_gaming_decision_forward() {
        for minimized in [false, true] {
            let mut shell = shell();
            gaming_decision(&mut shell);
            let replies = replies(&shell);
            shell.hidden = !minimized;
            let ctx = egui::Context::default();
            native(&shell, crate::app_menu::NativeRequest::SystemQuit);
            // Only the logic pass runs while the window cannot be seen.
            let commands = pass(&ctx, Default::default(), |ctx| {
                shell.visibility(ctx, minimized)
            })
            .0;
            assert!(shell.exit_requested, "{minimized}");
            assert!(shell.image_library.needs_decision());
            assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
            assert!(commands.contains(&egui::ViewportCommand::Focus));
            assert!(replies.borrow().is_empty());
        }
    }

    #[test]
    fn system_quit_of_a_hidden_window_asks_from_the_logic_pass() {
        let mut shell = shell();
        let replies = replies(&shell);
        shell.hidden = true;
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        let commands = pass(&ctx, Default::default(), |ctx| shell.visibility(ctx, false)).0;
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
        assert!(replies.borrow().is_empty());
        // Shown again, the user cancels from the full pass.
        shell.hidden = false;
        let commands = pass(&ctx, Default::default(), |ctx| {
            shell.answer_stop_prompt(false, ctx);
            shell.input(ctx, false);
            shell.window_close(ctx);
        })
        .0;
        assert_eq!(*replies.borrow(), [false]);
        assert!(!commands.contains(&egui::ViewportCommand::Close));
    }

    #[test]
    fn system_quit_with_a_failed_stop_keeps_running_and_shows_the_error() {
        let temp = machine_file::test_support::TempDir::new("shell-system-quit-failed-stop");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled().persistence[0].fail_output();
        let replies = replies(&shell);
        let ctx = egui::Context::default();
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        quit_pass(&mut shell, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        let commands = pass(&ctx, Default::default(), |ctx| {
            shell.answer_stop_prompt(true, ctx);
            shell.input(ctx, false);
            shell.window_close(ctx);
        })
        .0;
        // Logout is cancelled so the unfinished journal is reported.
        assert_eq!(*replies.borrow(), [false]);
        assert!(shell.presenter.is_none());
        assert_eq!(
            shell.dialog.as_ref().map(|(title, _)| *title),
            Some("Machine error")
        );
        assert!(shell.errors.borrow().is_some());
        assert!(!shell.exit_requested);
        assert!(!shell.events.borrow().native.terminating());
        assert!(commands.contains(&egui::ViewportCommand::CancelClose));
        assert!(!commands.contains(&egui::ViewportCommand::Close));
        // Closing the error does not quit; a later quit stops nothing and terminates.
        shell.dialog = None;
        assert!(quit_pass(&mut shell, &ctx).is_empty());
        native(&shell, crate::app_menu::NativeRequest::SystemQuit);
        quit_pass(&mut shell, &ctx);
        assert_eq!(*replies.borrow(), [false, true]);
    }

    #[test]
    fn command_chords_taken_by_the_application_menu_never_reach_the_guest() {
        use KeyCode::*;
        // AppKit gives Command+H, Command+Option+H and Command+Q to the menu,
        // so the window sees the modifiers and only the letter's release, with
        // Command still down or already released.
        for command_first in [false, true] {
            for (option, letter, logical) in
                [(false, KeyH, "h"), (true, KeyH, "h"), (false, KeyQ, "q")]
            {
                let mut shell = keyboard_shell();
                shell.macos_shortcuts = true;
                let ctx = egui::Context::default();
                shell.key(SuperLeft, Pressed, false, &ctx);
                if option {
                    shell.key(AltLeft, Pressed, false, &ctx);
                }
                if command_first {
                    shell.key(SuperLeft, Released, false, &ctx);
                }
                logical_key(&mut shell, letter, logical, Released, &ctx);
                if option {
                    shell.key(AltLeft, Released, false, &ctx);
                }
                if !command_first {
                    shell.key(SuperLeft, Released, false, &ctx);
                }
                // Neither the letter nor an Option pressed under Command
                // reaches the guest, so no Alt tap opens a DOS menu bar.
                assert!(
                    take_guest_scancodes(&mut shell).is_empty(),
                    "{letter:?} {option} {command_first}"
                );
                assert_eq!(shell.stop_prompt, None);
                assert!(shell.dialog.is_none());
                // Later guest typing is unaffected.
                shell.key(KeyA, Pressed, false, &ctx);
                shell.key(KeyA, Released, false, &ctx);
                assert_eq!(take_guest_scancodes(&mut shell), [0x1E, 0x9E]);
            }
        }
    }

    #[test]
    fn alt_held_before_command_stays_guest_input_through_the_chord() {
        use KeyCode::*;
        let mut shell = keyboard_shell();
        shell.macos_shortcuts = true;
        let ctx = egui::Context::default();
        shell.key(AltLeft, Pressed, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0x38]);
        shell.key(SuperLeft, Pressed, false, &ctx);
        logical_key(&mut shell, KeyH, "h", Released, &ctx);
        shell.key(AltLeft, Released, false, &ctx);
        shell.key(SuperLeft, Released, false, &ctx);
        assert_eq!(take_guest_scancodes(&mut shell), [0xB8]);
    }

    /// Import a one-disk 360K installation set into the fixture's store.
    fn modal_frames(ctx: &egui::Context, shell: &mut Shell, done: impl Fn(&Shell) -> bool) {
        for _ in crate::test_wait::rounds() {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                shell.media_modals(ui.ctx());
            });
            output.textures_delta.clear();
            if done(shell) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the media dialogs never finished");
    }

    /// A set the picker chose that the library then refuses (here: no store)
    /// only says why; the guest keeps running and the drives take disks.
    #[test]
    fn a_refused_picker_insert_keeps_the_machine_running_and_says_why() {
        let temp = machine_file::test_support::TempDir::new("refused-insert");
        let ctx = egui::Context::default();
        let mut shell = media_shell();
        shell.preferences.store_root = Some(temp.path().join("no-store"));
        shell.args.machine = temp.path().join("machine.toml");
        assert!(shell.presenter.as_ref().unwrap().takes_media());
        shell.apply_picker(crate::media_picker::tests::set_outcome(0));
        assert!(shell.insert_dialog.as_ref().is_some_and(|d| d.automatic));
        modal_frames(&ctx, &mut shell, |shell| shell.insert_dialog.is_none());
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(presenter.running && presenter.takes_media());
        assert!(shell.errors.borrow().is_none());
        let (title, text) = shell.dialog.clone().expect("the reason");
        assert_eq!(title, "Insert disk");
        assert!(
            !text.is_empty() && text != "Choose an installation set",
            "{text}"
        );
        assert!(shell.stop());
    }

    /// Browse for an image file… shows only that file's Insert floppy step;
    /// a disk of the drive's catalogue goes in at once, also while halted.
    #[test]
    fn browse_shows_the_files_own_step_and_a_catalogue_disk_goes_in_now() {
        let temp = machine_file::test_support::TempDir::new("picker-browse");
        let mut shell = media_shell();
        let file = temp.path().join("game.img");
        std::fs::write(&file, vec![0; 737_280]).unwrap();
        shell.browse_into(0, &file);
        let draft = shell.insert_dialog.as_ref().expect("the file's step");
        assert!(draft.browsed_only);
        let chosen = draft
            .selected
            .and_then(|i| draft.slot.entries.get(i))
            .unwrap();
        assert_eq!(chosen.medium.name, "game.img");
        shell.insert_dialog = None;
        shell.browse_into(0, &temp.path().join("missing.img"));
        assert!(shell.insert_dialog.is_none());
        assert_eq!(shell.dialog.as_ref().map(|(t, _)| *t), Some("Insert disk"));
        shell.dialog = None;
        shell.presenter.as_mut().unwrap().halted = true;
        shell.apply_picker(crate::media_picker::Outcome::Entry { unit: 0, index: 1 });
        assert_eq!(
            shell.presenter.as_ref().unwrap().media_events,
            [(
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Insert(1)
            )]
        );
        shell.presenter.as_mut().unwrap().halted = false;
        assert!(shell.stop());
    }

    fn machine_shell(f: &crate::library_test_support::Fixture) -> (Shell, String) {
        let mut shell = shell();
        shell.stop();
        shell.args.options.allow_unlisted_firmware = true;
        shell.preferences.store_root = Some(f.temp.path().join("no-store"));
        let id = pc_image::machine::create(
            &f.store,
            &f.row.descriptor,
            &f.row.reference,
            Some("DOS"),
            None,
        )
        .unwrap();
        shell.image_library =
            crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
        shell
            .image_library
            .pump_until(|view| view.machine_rows().iter().any(|row| row.id() == id));
        let row = shell.image_library.machine_rows()[0].image_row();
        shell.image_library.show_machine(id.clone(), row);
        (shell, id)
    }

    #[test]
    fn core_launch_preserves_360k_media_in_a_1200k_drive() {
        mixed_capacity_core_launch("1.2M", assembly::FloppyGeometry::DD_360K);
    }

    #[test]
    fn core_launch_preserves_720k_media_in_a_1440k_drive() {
        mixed_capacity_core_launch("1.44M", assembly::FloppyGeometry::DD_720K);
    }

    fn mixed_capacity_core_launch(drive: &str, medium: assembly::FloppyGeometry) {
        for no_compress in [false, true] {
            let mut f = crate::library_test_support::Fixture::new();
            let path = f.temp.path().join("machine.toml");
            let config = std::fs::read_to_string(&path).unwrap();
            std::fs::write(
                &path,
                config.replace("type='360K'", &format!("type='{drive}'")),
            )
            .unwrap();
            for name in ["one.img", "two.img"] {
                std::fs::write(
                    f.temp.path().join(name),
                    vec![0; medium.image_bytes() as usize],
                )
                .unwrap();
            }
            f.row.descriptor = pc_image::test_support::with_synthetic_firmware(|| {
                pc_image::import_machine(
                    &f.store,
                    &path,
                    "test:mixed-capacity",
                    &pc_image::ImportOptions {
                        no_compress,
                        ..Default::default()
                    },
                )
            })
            .unwrap()
            .manifest;
            f.row.reference = "test:mixed-capacity".into();
            let (mut shell, id) = machine_shell(&f);
            let ctx = egui::Context::default();
            shell.command(Command::Start, &ctx);
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.presenter.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
            let chips = shell.drive_views();
            assert_eq!(chips[0].medium.as_ref().unwrap().format, Some(medium));
            let status = status_pass(&ctx, &mut shell, 1600.0, Vec::new());
            assert!(status
                .labels
                .contains(&format!("Drive {}", chips[0].chip_text())));
            shell.command(Command::Insert(0), &ctx);
            let picker = shell.picker.as_ref().unwrap();
            assert_eq!(picker.machine().drives[0].catalogue.len(), 2);
            for disk in &picker.machine().drives[0].catalogue {
                assert_eq!(disk.format, medium);
            }
            shell.picker = None;
            shell.command(Command::CatalogueDisk(0, 1), &ctx);
            shell.tick(&ctx, false, std::time::Duration::ZERO);
            assert_eq!(shell.floppy_inserted()[0], Some(1));
            assert!(shell.stop());
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.image_library.active.is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(shell.image_library.active.is_none());
            let kept = crate::kept_catalogue::read(&f.store.workspace_path(&id).unwrap()).unwrap();
            assert_eq!(kept[0].disk.len(), 2);
            let expected = crate::media_dialog::geometry_name(medium);
            for disk in &kept[0].disk {
                assert!(
                    matches!(disk,
                        crate::kept_catalogue::KeptDisk::Own { format: Some(format), .. }
                        if format == &expected
                    ),
                    "{disk:?}"
                );
            }
            shell.image_library.shutdown();
        }
    }

    #[test]
    fn created_and_kept_disks_start_through_the_core_and_survive_a_gaming_power_cycle() {
        use crate::created_media::{CreatedDisk, CreatedInsert};
        use sha2::Digest as _;
        let f = crate::library_test_support::Fixture::new();
        let (mut shell, id) = machine_shell(&f);
        shell.args.speaker_trace = true;
        let diagnostics = f.temp.path().join("diagnostics");
        shell.args.diagnostics = Ok(diagnostics.clone());
        let workspace = f.store.workspace_path(&id).unwrap();
        std::fs::create_dir_all(workspace.join("media/created")).unwrap();
        let bytes = vec![0x31; 360 * 1024];
        let digest: [u8; 32] = sha2::Sha256::digest(&bytes).into();
        let sha256 = crate::created_media::hex(&digest);
        let base = workspace.join(format!("media/created/{sha256}.img"));
        std::fs::write(&base, &bytes).unwrap();
        crate::created_media::set_pending(
            &workspace,
            "A:",
            Some(CreatedInsert {
                drive: "A:".into(),
                title: "Created".into(),
                disks: vec![CreatedDisk {
                    sha256: sha256.clone(),
                    name: "Created disk".into(),
                }],
                journal: true,
                first: Some(0),
            }),
        )
        .unwrap();
        let ctx = egui::Context::default();
        let wait_running = |shell: &mut Shell| {
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.presenter.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
            assert!(shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled_ref()
                .observation
                .speaker_trace_enabled());
            let drawn = draw(&ctx, shell, Vec::new());
            for (label, disabled) in [
                ("Start", true),
                ("Stop", false),
                ("A:", false),
                ("B:", true),
            ] {
                assert!(
                    drawn.controls.contains(&(label.into(), disabled)),
                    "{:?}",
                    drawn.controls
                );
            }
            let drives = shell.drive_views();
            assert_eq!(drives.len(), 2);
            let status = status_pass(&ctx, shell, 1600.0, Vec::new());
            for (drive, letter) in drives.iter().zip(['A', 'C']) {
                assert_eq!(drive.letter(), letter);
                assert!(
                    status
                        .labels
                        .contains(&format!("Drive {}", drive.chip_text())),
                    "{:?}",
                    status.labels
                );
            }
        };
        let stop = |shell: &mut Shell| {
            assert!(shell.stop());
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.image_library.active.is_none() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(shell.image_library.active.is_none());
        };
        shell.command(Command::Start, &ctx);
        wait_running(&mut shell);
        for number in 1..=2 {
            let drawn = open_diagnostics_menu(&ctx, &mut shell);
            click(
                &ctx,
                &mut shell,
                label_rect(&drawn.text, "Save machine report").center(),
            );
            let path = diagnostics.join(format!("diag-{number}.txt"));
            assert_eq!(
                shell.message,
                format!("Machine report saved to {}", path.display())
            );
            let report = std::fs::read_to_string(path).unwrap();
            assert!(
                report.starts_with("machine diagnostic report\n"),
                "{report}"
            );
            assert!(report.contains("\ndelivery the last "), "{report}");
        }
        let drawn = draw(&ctx, &mut shell, Vec::new());
        assert!(
            click_toolbar(&ctx, &mut shell, label_rect(&drawn.text, "100%").center()).is_empty()
        );
        let drawn = draw(&ctx, &mut shell, Vec::new());
        let commands = click_toolbar(
            &ctx,
            &mut shell,
            label_rect(&drawn.text, "Unlimited").center(),
        );
        assert_eq!(commands, [Command::Speed(crate::pacing::Speed::Unlimited)]);
        shell.command(commands[0], &ctx);
        assert_eq!(shell.selected_speed(), crate::pacing::Speed::Unlimited);
        assert_eq!(shell.toolbar_speed(), crate::pacing::Speed::Unlimited);
        assert_eq!(marked(&speed_menu(&ctx, &mut shell)), ["Unlimited"]);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(presenter.pacing.target(std::time::Duration::ZERO, 0), None);
        let config = &shell.presenter.as_ref().unwrap().media_config;
        let fixed = config
            .drives
            .iter()
            .find(|drive| drive.controller == assembly::DeviceId::Ide)
            .unwrap();
        let assembly::ImageSource::File(fixed_base) = &fixed.image else {
            panic!("fixed disk base");
        };
        assert!(fixed_base.is_file());
        let assembly::WriteMode::Journal {
            path: fixed_journal,
            ..
        } = &fixed.write
        else {
            panic!("fixed disk journal");
        };
        assert!(fixed_journal.starts_with(&workspace) && fixed_journal.is_file());
        let entry = &config.drives[0].media.as_ref().unwrap().entries[0];
        assert_eq!(entry.name, "Created disk");
        assert_eq!(entry.expected_sha256, Some(digest));
        assert_eq!(entry.image, assembly::ImageSource::File(base.clone()));
        let journal = workspace.join(format!("media/{sha256}.upfj"));
        assert!(journal.is_file());
        // A host disk added while running becomes the kept selection at restart.
        let host = f.temp.path().join("kept.img");
        std::fs::write(&host, vec![0x52; 360 * 1024]).unwrap();
        shell.browse_into(0, &host);
        shell.confirm_insert();
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        stop(&mut shell);
        let sealed = std::fs::read(&journal).unwrap();
        shell.command(Command::Start, &ctx);
        wait_running(&mut shell);
        let inserted = shell.floppy_inserted();
        let catalogue = shell.catalogue(0, inserted).unwrap();
        assert_eq!(catalogue.disks.last().unwrap().name, "kept.img");
        assert_eq!(catalogue.in_drive, Some(catalogue.disks.len() - 1));
        assert_eq!(std::fs::read(&journal).unwrap(), sealed);
        stop(&mut shell);
        shell.command(Command::SaveImage, &ctx);
        assert!(shell.image_library.has_dialog());
        shell.image_library.close_dialogs();
        shell
            .image_library
            .send(crate::library_worker::Request::Machine(
                crate::library_worker::MachineJob::Commit {
                    id: id.clone(),
                    keep: true,
                    target: library_model::save_image::Target {
                        reference: "test:core-saved".into(),
                        expected: None,
                        title: None,
                        flatten: false,
                    },
                },
            ));
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if f.store.tag_descriptor("test:core-saved").is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            f.store.tag_descriptor("test:core-saved").is_ok(),
            "{:?}",
            shell.dialog
        );
        shell.command(Command::GamingMode, &ctx);
        wait_running(&mut shell);
        let copy = shell.image_library.active.as_ref().unwrap().1.clone();
        assert_ne!(copy, id);
        shell.command(Command::PowerCycle, &ctx);
        wait_running(&mut shell);
        assert_eq!(shell.image_library.active.as_ref().unwrap().1, copy);
        let inserted = shell.floppy_inserted();
        assert_eq!(
            shell
                .catalogue(0, inserted)
                .unwrap()
                .disks
                .last()
                .unwrap()
                .name,
            "kept.img"
        );
        assert_eq!(std::fs::read(&base).unwrap(), bytes);
        shell.retain_workspace_on_stop = true;
        stop(&mut shell);
        shell.image_library.shutdown();
    }

    /// A library machine's drive catalogue survives Stop: a set's disk
    /// Insert… put in from the library and an image file Browse… put in
    /// come back at the next Start in the same order, with the same short
    /// names and write settings, and the disk that was in the drive is in it
    /// again. Stopped, the Catalogue submenu lists them as waiting for that
    /// Start, the one in the drive "at Start", and choosing another makes it
    /// the one that goes in then.
    #[test]
    fn a_library_machine_keeps_its_catalogue_across_stop_and_start() {
        let f = crate::library_test_support::Fixture::new();
        f.import_set();
        let (mut shell, id) = machine_shell(&f);
        // Insert… from the library reads this store.
        shell.preferences.store_root = Some(f.store.root().to_owned());
        let ctx = egui::Context::default();
        let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
        let start = |shell: &mut Shell| {
            shell.command(Command::Start, &ctx);
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.presenter.is_some() {
                    break;
                }
                pause();
            }
            assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
            // The first run boundary takes the queued media changes.
            shell.tick(&ctx, false, std::time::Duration::ZERO);
        };
        let stop = |shell: &mut Shell| {
            assert!(shell.stop());
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.image_library.active.is_none() {
                    break;
                }
                pause();
            }
            assert!(shell.image_library.active.is_none());
        };
        let listed = |shell: &mut Shell| {
            let inserted = shell.floppy_inserted();
            let catalogue = shell.catalogue(0, inserted).unwrap();
            let disks: Vec<(String, String)> = catalogue
                .disks
                .iter()
                .map(|disk| (disk.name.clone(), disk.write.clone()))
                .collect();
            (disks, catalogue.in_drive)
        };
        start(&mut shell);
        let own = listed(&mut shell).0;
        // Insert… from the library, keeping its writes in the machine.
        shell.command(Command::InsertFile(0), &ctx);
        shell.open_insert_library();
        for _ in crate::test_wait::rounds() {
            let library = shell
                .insert_dialog
                .as_mut()
                .unwrap()
                .library
                .as_mut()
                .unwrap();
            assert_eq!(library.poll(), None);
            if !library.is_busy() {
                break;
            }
            pause();
        }
        let library = shell
            .insert_dialog
            .as_mut()
            .unwrap()
            .library
            .as_mut()
            .unwrap();
        library.selected = library
            .rows
            .iter()
            .position(|row| row.tag == "disks-set:r1");
        library.journal = true;
        for _ in crate::test_wait::rounds() {
            shell.confirm_insert();
            if shell.insert_dialog.is_none() {
                break;
            }
            assert_eq!(shell.insert_dialog.as_ref().unwrap().error, None);
            pause();
        }
        // An image file of this computer, write-protected.
        let host = f.temp.path().join("game.img");
        std::fs::write(&host, vec![0x5a; 360 * 1024]).unwrap();
        shell.browse_into(0, &host);
        shell.confirm_insert();
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        let before = listed(&mut shell);
        let mut expected = own.clone();
        expected.extend([
            ("Session disks".to_owned(), "changes kept in DOS".to_owned()),
            ("game.img".to_owned(), "write-protected".to_owned()),
        ]);
        assert_eq!(before, (expected.clone(), Some(own.len() + 1)));
        stop(&mut shell);
        // The worker keeps the catalogue before it finishes the session.
        let workspace = f.store.workspace_path(&id).unwrap();
        assert_eq!(
            crate::kept_catalogue::read(&workspace).unwrap()[0]
                .disk
                .len(),
            expected.len()
        );
        // Stopped, the catalogue waits for the next Start.
        shell
            .image_library
            .pump_until(|view| view.machine_rows().iter().any(|row| !row.kept.is_empty()));
        let waiting = shell.catalogue(0, [None, None]).unwrap();
        assert!(waiting.kept);
        assert_eq!(
            waiting
                .disks
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            waiting.pending,
            Some(crate::catalogue_menu::Pending::Entry(own.len() + 1))
        );
        assert_eq!(
            waiting.rows()[0],
            crate::catalogue_menu::Row::Heading(format!(
                "For A: at the next Start, in order · {} disks",
                expected.len()
            ))
        );
        assert!(!waiting.rows().contains(&crate::catalogue_menu::Row::Note(
            crate::catalogue_menu::OWN_DISKS_AT_START.into()
        )));
        shell.command(Command::Insert(0), &ctx);
        let picker = shell.picker.take().expect("the media picker");
        assert_eq!(picker.drive().unwrap().holds, "game.img at Start");
        assert_eq!(picker.drive().unwrap().catalogue, waiting.disks);
        // All N disks… opens This machine on the disk waiting for Start.
        shell.open_catalogue_picker(0);
        let picker = shell.picker.take().expect("the media picker");
        assert_eq!(picker.source, crate::media_picker::Source::Machine);
        assert_eq!(
            picker.insert(),
            Some(crate::media_picker::Outcome::Entry {
                unit: 0,
                index: own.len() + 1
            })
        );
        // A disk chosen for Start from the library leads, and the kept disks
        // follow it without that disk a second time; choosing a kept disk
        // takes the chosen one out again.
        let set = shell
            .image_library
            .picker_sets()
            .into_iter()
            .find(|set| set.reference == "disks-set:r1")
            .unwrap();
        let insert = image_store::machine::PendingInsert {
            drive: "A:".into(),
            set: set.image.clone(),
            reference: "disks-set:r1".into(),
            medium: set.disks[0].id.clone(),
            all: false,
            journal: true,
        };
        shell.image_library.set_pending(&id, "A:", Some(insert));
        let names = |catalogue: &crate::catalogue_menu::Catalogue| -> Vec<String> {
            catalogue.disks.iter().map(|d| d.name.clone()).collect()
        };
        let both = shell.catalogue(0, [None, None]).unwrap();
        let mut leading = vec!["Session disks".to_owned()];
        leading.extend(own.iter().map(|(name, _)| name.clone()));
        leading.push("game.img".into());
        assert_eq!(names(&both), leading);
        assert_eq!(both.pending, Some(crate::catalogue_menu::Pending::Entry(0)));
        let game = both.disks.last().unwrap().index;
        shell.command(Command::CatalogueDisk(0, game), &ctx);
        shell.image_library.pump_until(|view| {
            view.machine_rows()
                .iter()
                .any(|row| row.id() == id && row.listing.machine.pending.is_empty())
        });
        assert!(pc_image::machine::get(&f.store, &id)
            .unwrap()
            .machine
            .pending
            .is_empty());
        assert_eq!(
            crate::kept_catalogue::read(&workspace).unwrap()[0].in_drive,
            Some(own.len() + 1)
        );
        start(&mut shell);
        assert_eq!(listed(&mut shell), before);
        // The set's disk is the library's again, with its tooltip.
        let set_disk = &shell.presenter.as_ref().unwrap().media_slots[0]
            .as_ref()
            .unwrap()
            .entries[own.len()];
        assert!(set_disk.source.is_some());
        assert!(set_disk
            .origin
            .as_deref()
            .is_some_and(|origin| origin.starts_with("Media set disks-set:r1 from")));
        // A disk put in just before Stop, before the next run boundary, is
        // the one kept in the drive.
        shell.command(Command::CatalogueDisk(0, 0), &ctx);
        stop(&mut shell);
        assert_eq!(
            crate::kept_catalogue::read(&workspace).unwrap()[0].in_drive,
            Some(0)
        );
        // Another kept disk chosen while stopped goes in at the next Start.
        shell.command(Command::CatalogueDisk(0, own.len()), &ctx);
        assert_eq!(
            shell.message,
            "Session disks goes into A: of DOS at its next Start."
        );
        shell.image_library.pump_until(|_| {
            crate::kept_catalogue::read(&workspace)
                .is_ok_and(|kept| kept[0].in_drive == Some(own.len()))
        });
        start(&mut shell);
        assert_eq!(listed(&mut shell), (expected, Some(own.len())));
        stop(&mut shell);
        shell.image_library.shutdown();
    }

    /// Pending disks of a stopped machine change one drive at a time: two
    /// inserts in a row keep both, even before the worker lists the machine
    /// again; the chips show them at Start; Take out again removes one.
    #[test]
    fn pending_disks_of_a_stopped_machine_change_one_drive_at_a_time() {
        let f = crate::library_test_support::Fixture::new();
        let set = f.import_set();
        let (mut shell, id) = machine_shell(&f);
        let insert = |drive: &str| image_store::machine::PendingInsert {
            drive: drive.into(),
            set: set.clone(),
            reference: "disks-set:r1".into(),
            medium: "disk-1".into(),
            all: false,
            journal: true,
        };
        for drive in ["A:", "B:"] {
            shell.apply_picker(crate::media_picker::Outcome::Pending {
                id: id.clone(),
                insert: insert(drive),
            });
        }
        // At once, before any listing: both drives hold their disk.
        let row = shell.image_library.selected_machine_row().unwrap().clone();
        assert_eq!(row.listing.machine.pending, [insert("A:"), insert("B:")]);
        let chips: Vec<_> = shell
            .drive_views()
            .iter()
            .map(crate::drive_chips::DriveView::chip_text)
            .collect();
        assert_eq!(
            chips.len(),
            1,
            "the machine has one floppy drive: {chips:?}"
        );
        assert!(
            chips[0].starts_with("A: ") && chips[0].ends_with(" · at Start"),
            "{chips:?}"
        );
        assert!(shell.command_allowed(Command::Eject(0)));
        let ctx = egui::Context::default();
        shell.command(Command::Eject(0), &ctx);
        // The worker answers the three changes in order, then lists again.
        shell.image_library.pump_until(|view| {
            view.status
                .as_deref()
                .is_some_and(|s| s.starts_with("Took the pending disk out of A:"))
                && view
                    .selected_machine_row()
                    .is_some_and(|row| row.listing.machine.pending == [insert("B:")])
        });
        assert_eq!(
            pc_image::machine::get(&f.store, &id)
                .unwrap()
                .machine
                .pending,
            [insert("B:")]
        );
        assert!(!shell.command_allowed(Command::Eject(0)));
        shell.image_library.shutdown();
    }

    /// Floppies inserted into an image before Start belong to the machine
    /// Start on that image makes, never to another machine of the image.
    #[test]
    fn floppies_inserted_into_an_image_never_go_into_another_machine_of_it() {
        let f = crate::library_test_support::Fixture::new();
        let (mut shell, id) = machine_shell(&f);
        let ctx = egui::Context::default();
        shell.image_library.show_source_image();
        for _ in crate::test_wait::rounds() {
            shell.poll_images(&ctx);
            if shell.image_library.readiness.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        shell.sync_pending_media();
        assert!(matches!(
            shell.pending_media.as_ref().map(|p| &p.owner),
            Some(crate::media_dialog::PendingOwner::Image(_))
        ));
        let extra = f.temp.path().join("extra.img");
        std::fs::write(&extra, vec![7; 360 * 1024]).unwrap();
        shell.browse_into(0, &extra);
        shell.confirm_insert();
        assert!(shell.pending_media.as_ref().unwrap().has_insert(0));
        // Selecting the image's existing machine drops them.
        let row = shell.image_library.machine_rows()[0].image_row();
        shell.image_library.show_machine(id.clone(), row.clone());
        shell.sync_pending_media();
        assert!(shell.pending_media.is_none());
        // The machine Start on the image made keeps them for its Start.
        shell.image_library.show_source_image();
        shell.sync_pending_media();
        shell.browse_into(0, &extra);
        shell.confirm_insert();
        shell.image_library.show_machine(id.clone(), row);
        shell.image_library.started_from_image = Some(id);
        shell.sync_pending_media();
        assert!(shell
            .pending_media
            .as_ref()
            .is_some_and(|p| p.has_insert(0)));
        shell.image_library.shutdown();
    }

    /// The menus, the toolbar and the keys offer nothing that reads the
    /// config of an image this release does not read, nor of a machine made
    /// from one (machine-images §5.6); the same selection of an image this
    /// release reads offers them.
    #[test]
    fn nothing_that_reads_a_newer_images_config_is_offered() {
        use crate::library_test_support::{tag_newer, warm, Fixture};
        use Command::*;
        let f = Fixture::new();
        tag_newer(
            &f.store,
            "test:source",
            "newer:1.0.0",
            Some("99.0.0"),
            false,
        );
        let cache = warm(&f.store);
        let row = |tag: &str| {
            let descriptor = f.store.tag_descriptor(tag).unwrap();
            library_model::listing::ImageRow {
                reference: tag.into(),
                aliases: Vec::new(),
                summary: cache.summary(&descriptor.digest).cloned(),
                descriptor,
                failure: None,
            }
        };
        let mut shell = shell();
        shell.stop();
        let gated = [
            NewFromImage,
            ImageSettings,
            ImageLayers,
            Settings,
            Layers,
            Push,
            SaveImage,
        ];
        for (tag, newer) in [("test:source", false), ("newer:1.0.0", true)] {
            shell.image_library.selected = Some(row(tag));
            for command in gated {
                assert_eq!(shell.command_allowed(command), !newer, "{tag}: {command:?}");
            }
        }
        let mut machine =
            crate::library_view::machines::tests::row("dos", "DOS", "a", &["games:1"]);
        for newer in [false, true] {
            machine.needs_newer = newer.then(|| "Needs unemupc 99.0.0 or newer".into());
            shell.image_library.set_machines(vec![machine.clone()]);
            shell.image_library.machine = Some("dos".into());
            shell.image_library.selected = Some(machine.image_row());
            for command in [Settings, Layers, SaveImage, Insert(0), CatalogueAll(0)] {
                assert_eq!(shell.command_allowed(command), !newer, "{command:?}");
            }
        }
        // Reached anyway (Insert into… of a set names the machine), the
        // picker does not open and says why.
        shell.open_picker(0, Some("dos"), None);
        assert!(shell.picker.is_none());
        assert_eq!(
            shell.dialog.as_ref().map(|(_, text)| text.as_str()),
            Some("Needs unemupc 99.0.0 or newer")
        );
        shell.image_library.shutdown();
    }

    /// A library key typed into a text field stays with the field.
    #[test]
    fn library_keys_never_act_while_a_text_field_has_the_keyboard() {
        use winit::event::ElementState::*;
        let mut shell = shell();
        shell.stop();
        let mut machine =
            crate::library_view::machines::tests::row("dos", "DOS", "a", &["games:1"]);
        machine.listing.machine.favorite = false;
        shell.image_library.set_machines(vec![machine.clone()]);
        shell.image_library.machine = Some("dos".into());
        shell.image_library.selected = Some(machine.image_row());
        let ctx = egui::Context::default();
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::unique("a text field")));
        assert!(ctx.egui_wants_keyboard_input());
        shell.key(KeyCode::F2, Pressed, false, &ctx);
        shell.key(KeyCode::F2, Released, false, &ctx);
        assert!(
            !shell.image_library.has_dialog(),
            "F2 stayed with the field"
        );
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::unique("a text field")));
        shell.key(KeyCode::F2, Pressed, false, &ctx);
        shell.key(KeyCode::F2, Released, false, &ctx);
        assert!(shell.image_library.has_dialog(), "Rename… opens");
        // Disabled items say why.
        let shell = self::shell();
        assert_eq!(
            shell.disabled_reason(Command::Rename),
            Some("Select a machine")
        );
        assert_eq!(shell.disabled_reason(Command::Stop), None);
    }

    fn import_disk_set(f: &crate::library_test_support::Fixture) {
        f.import_set();
    }

    #[test]
    fn image_sessions_journal_library_disks_in_their_own_workspace() {
        use crate::library_test_support::Fixture;
        use crate::library_worker::WorkspaceKind;
        let pause = || std::thread::sleep(std::time::Duration::from_millis(5));
        for kind in [WorkspaceKind::Regular, WorkspaceKind::Disposable] {
            let f = Fixture::new();
            import_disk_set(&f);
            let ctx = egui::Context::default();
            let mut shell = shell();
            shell.stop();
            shell.args.options.allow_unlisted_firmware = true;
            shell.preferences.store_root = Some(f.store.root().to_owned());
            shell.image_library =
                crate::library_view::LibraryView::synthetic(f.store.root().to_owned());
            shell.image_library.selected = Some(f.row.clone());
            shell.poll_images(&ctx);
            shell.command(
                if kind.disposable() {
                    Command::GamingMode
                } else {
                    Command::Start
                },
                &ctx,
            );
            for _ in crate::test_wait::rounds() {
                shell.poll_images(&ctx);
                if shell.presenter.is_some() {
                    break;
                }
                pause();
            }
            assert!(shell.presenter.is_some(), "{:?}", shell.dialog);
            let name = shell.image_library.active.as_ref().unwrap().1.clone();
            let media = f.store.workspace_path(&name).unwrap().join("media");
            assert_eq!(shell.library_media_dir().unwrap(), media);

            shell.command(Command::InsertFile(0), &ctx);
            shell.open_insert_library();
            for _ in crate::test_wait::rounds() {
                let library = shell
                    .insert_dialog
                    .as_mut()
                    .unwrap()
                    .library
                    .as_mut()
                    .unwrap();
                assert_eq!(library.poll(), None);
                if !library.is_busy() {
                    break;
                }
                pause();
            }
            let library = shell
                .insert_dialog
                .as_mut()
                .unwrap()
                .library
                .as_mut()
                .unwrap();
            assert_eq!(library.workspace, media);
            let row = library
                .rows
                .iter()
                .position(|row| row.tag == "disks-set:r1")
                .unwrap();
            library.selected = Some(row);
            library.journal = true;
            for _ in crate::test_wait::rounds() {
                shell.confirm_insert();
                if shell.insert_dialog.is_none() {
                    break;
                }
                assert_eq!(shell.insert_dialog.as_ref().unwrap().error, None);
                pause();
            }
            let presenter = shell.presenter.as_mut().unwrap();
            let entry = presenter.media_slots[0]
                .as_ref()
                .unwrap()
                .entries
                .last()
                .unwrap()
                .clone();
            let assembly::WriteMode::Journal { path: journal, .. } = &entry.medium.write else {
                panic!("journal");
            };
            assert!(journal.starts_with(&media), "{}", journal.display());
            assert!(!journal.starts_with(f.store.root().join("images")));
            presenter.media_events.push((
                unemu_core_api::SlotId(0),
                unemu_core_api::MediaAction::Eject,
            ));
            presenter.apply_media_events().unwrap();
            assert!(journal.is_file());

            // The dialog names where the journal lives.
            shell.command(Command::InsertFile(0), &ctx);
            let rows = shell.insert_dialog.as_ref().unwrap().slot.entries.len();
            shell.insert_dialog.as_mut().unwrap().select(Some(rows - 1));
            let mut text = Vec::new();
            for _ in 0..2 {
                text = pass(&ctx, Default::default(), |ctx| shell.media_modals(ctx)).1;
            }
            assert!(shows(
                &text,
                if kind.disposable() {
                    "Journal in this Gaming-mode workspace; Discard removes it"
                } else {
                    "Journal in this image's workspace"
                }
            ));
            shell.insert_dialog = None;

            shell.command(Command::Stop, &ctx);
            if kind.disposable() {
                assert!(shell.image_library.needs_decision());
                shell
                    .image_library
                    .send(crate::library_view::Request::Discard(name.clone()));
                let workspace = f.store.workspace_path(&name).unwrap();
                for _ in crate::test_wait::rounds() {
                    shell.poll_images(&ctx);
                    if !workspace.exists() {
                        break;
                    }
                    pause();
                }
                assert!(!workspace.exists());
                assert!(!journal.exists());
            } else {
                for _ in crate::test_wait::rounds() {
                    shell.poll_images(&ctx);
                    if shell.image_library.active.is_none() {
                        break;
                    }
                    pause();
                }
                assert!(shell.image_library.active.is_none());
                assert!(journal.is_file());
            }
            shell.image_library.shutdown();
        }
    }

    /// Every place `label` is drawn, by its rect.
    fn rects_of(text: &[(String, egui::Rect)], label: &str) -> Vec<egui::Rect> {
        text.iter()
            .filter(|(shown, _)| shown == label)
            .map(|(_, rect)| *rect)
            .collect()
    }

    /// The menu item `label` with its mark: the mark's rect and the label's.
    fn mark_beside(
        text: &[(String, egui::Rect)],
        mark: &str,
        label: &str,
    ) -> (egui::Rect, egui::Rect) {
        let marks = rects_of(text, mark);
        rects_of(text, label)
            .into_iter()
            .find_map(|at| {
                marks
                    .iter()
                    .find(|m| (m.center().y - at.center().y).abs() < 3.0 && m.right() <= at.left())
                    .map(|m| (*m, at))
            })
            .unwrap_or_else(|| panic!("{label} has no {mark} beside it in {text:?}"))
    }

    #[test]
    fn machine_and_media_menus_carry_the_mockup_marks() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        configure_context(&ctx);
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(
            &ctx,
            &mut shell,
            label_rect(&drawn.text, "Machine").center(),
        );
        let menu = draw(&ctx, &mut shell, Vec::new());
        let mut lefts = Vec::new();
        for (mark, label) in [("▶", "Start"), ("Ⅱ", "Pause"), ("■", "Stop")] {
            let (_, at) = mark_beside(&menu.text, mark, label);
            lefts.push(at.left());
        }
        // Items without a mark keep the column, so every label lines up.
        let unmarked = rects_of(&menu.text, "Power cycle")[0];
        assert!(lefts
            .iter()
            .all(|left| (left - unmarked.left()).abs() < 0.5));
        egui::Popup::close_all(&ctx);
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "Media").center());
        let menu = draw(&ctx, &mut shell, Vec::new());
        mark_beside(&menu.text, "⏏", "Eject A:");
        egui::Popup::close_all(&ctx);
    }

    #[test]
    fn the_toolbar_draws_the_mockup_icons_and_sets_ctrl_alt_del_as_code() {
        let shell = shell();
        let ctx = egui::Context::default();
        configure_context(&ctx);
        let mut commands = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            shell.toolbar(ui, &mut commands);
        });
        output.textures_delta.clear();
        let mut fills = Vec::new();
        let mut cad = None;
        for clipped in &output.shapes {
            match &clipped.shape {
                egui::Shape::Path(path) => fills.push(path.fill),
                egui::Shape::Rect(rect) => fills.push(rect.fill),
                egui::Shape::Text(text) if text.galley.text() == "Ctrl+Alt+Del" => {
                    cad = Some(text.galley.job.sections[0].format.font_id.clone());
                }
                _ => (),
            }
        }
        output.drop_without_applying_deltas();
        // The green Start, the red Stop, the floppy and the blue top layer.
        for colour in [
            Color32::from_rgb(0x2c, 0x8a, 0x3a),
            Color32::from_rgb(0xb2, 0x3a, 0x2b),
            Color32::from_rgb(0x3a, 0x3f, 0x4a),
            Color32::from_rgb(0x4b, 0x79, 0xc1),
        ] {
            // A disabled button fades its icon; undo the fade to compare.
            let near = |fill: &Color32| {
                let fill = fill.to_opaque();
                [fill.r(), fill.g(), fill.b()]
                    .into_iter()
                    .zip([colour.r(), colour.g(), colour.b()])
                    .all(|(a, b)| a.abs_diff(b) <= 3)
            };
            assert!(fills.iter().any(near), "{colour:?} not painted");
        }
        assert_eq!(cad, Some(egui::FontId::monospace(11.0)));
    }

    #[test]
    fn the_classic_title_strip_shows_the_application_icon() {
        let ctx = egui::Context::default();
        let strip = |icon: bool| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                if icon {
                    window_title(ui, "unemu");
                } else {
                    gradient_title(ui, "Settings");
                }
            });
            output.textures_delta.clear();
            let shapes: Vec<_> = output.shapes.iter().map(|c| c.shape.clone()).collect();
            output.drop_without_applying_deltas();
            shapes
        };
        let screen = Color32::from_rgb(0x0b, 0x2a, 0x5a);
        let window = strip(true);
        assert!(window.iter().any(|shape| matches!(
            shape,
            egui::Shape::Rect(rect) if rect.fill == screen && rect.rect.size() == Vec2::new(10.0, 5.0)
        )));
        let text_left = |shapes: &[egui::Shape]| {
            shapes
                .iter()
                .find_map(|shape| match shape {
                    egui::Shape::Text(text) => Some(text.pos.x),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(text_left(&window), 28.0);
        // Dialog title strips keep their plain title.
        let dialog = strip(false);
        assert!(!dialog
            .iter()
            .any(|shape| matches!(shape, egui::Shape::Rect(rect) if rect.fill == screen)));
        assert_eq!(text_left(&dialog), 7.0);
    }

    /// Text fields and mini tables take the mock-ups' field colour and lists
    /// their stripe colour in both skins: egui's own dark field is near black.
    #[test]
    fn every_skin_takes_the_mock_up_field_and_stripe_colours() {
        for classic in [false, true] {
            for (dark, field, stripe) in [
                (
                    false,
                    Color32::from_rgb(0xff, 0xff, 0xff),
                    Color32::from_rgb(0xf3, 0xf1, 0xea),
                ),
                (
                    true,
                    Color32::from_rgb(0x26, 0x28, 0x2b),
                    Color32::from_rgb(0x2c, 0x2e, 0x32),
                ),
            ] {
                let visuals = skin_visuals(classic, dark);
                let skin = format!("classic {classic}, dark {dark}");
                assert_eq!(visuals.extreme_bg_color, field, "{skin}");
                assert_eq!(visuals.text_edit_bg_color(), field, "{skin}");
                assert_eq!(visuals.faint_bg_color, stripe, "{skin}");
            }
        }
    }

    /// The mock-ups' colour tokens of one theme, from the CSS at the top of
    /// `backlog/docs/design/mockups/machines.html`.
    struct MockUp {
        face: Color32,
        face_2: Color32,
        ink: Color32,
        muted: Color32,
        sel: Color32,
        link: Color32,
        lo: Color32,
        warn: Color32,
        /// `--bad`, the error colour of the mock-ups that show errors, such
        /// as `new-machine.html`.
        bad: Color32,
    }
    fn mock_up(dark: bool) -> MockUp {
        let rgb = |hex: u32| Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        if dark {
            MockUp {
                face: rgb(0x34363a),
                face_2: rgb(0x3e4045),
                ink: rgb(0xe6e4df),
                muted: rgb(0xa9a6a0),
                sel: rgb(0x3c64b0),
                link: rgb(0xa9c6ff),
                lo: rgb(0x1f2023),
                warn: rgb(0xffcf73),
                bad: rgb(0xff9d92),
            }
        } else {
            MockUp {
                face: rgb(0xd6d3cb),
                face_2: rgb(0xe7e4dc),
                ink: rgb(0x1d1c1a),
                muted: rgb(0x5b5852),
                sel: rgb(0x22468a),
                link: rgb(0x1b4a96),
                lo: rgb(0x8d8a83),
                warn: rgb(0x8a5300),
                bad: rgb(0x9b1c1c),
            }
        }
    }

    /// Warnings and errors take the mock-ups' warn and bad colours in both
    /// skins, which share every colour token.
    #[test]
    fn every_skin_takes_the_mock_up_warn_and_error_colours() {
        for classic in [false, true] {
            for dark in [false, true] {
                let token = mock_up(dark);
                let visuals = skin_visuals(classic, dark);
                let skin = format!("classic {classic}, dark {dark}");
                assert_eq!(visuals.warn_fg_color, token.warn, "{skin}");
                assert_eq!(visuals.error_fg_color, token.bad, "{skin}");
            }
        }
    }

    /// The mock-ups' modern skin shares every colour with the classic one and
    /// changes only the corners and the outlines: panels, dialogs and resting
    /// buttons take the face colour, hovered and open buttons and the boxes
    /// of check boxes the second face colour, text the ink and muted
    /// colours, while corners stay rounded and outlines flat.
    #[test]
    fn the_modern_skin_takes_the_mock_up_colours_and_keeps_its_shape() {
        for dark in [false, true] {
            let token = mock_up(dark);
            let visuals = skin_visuals(false, dark);
            let widgets = &visuals.widgets;
            let theme = format!("dark {dark}");
            assert_eq!(visuals.panel_fill, token.face, "{theme}");
            assert_eq!(visuals.window_fill, token.face, "{theme}");
            assert_eq!(widgets.noninteractive.bg_fill, token.face, "{theme}");
            // `.btn` rests on the face and turns to the second face on hover
            // and while its menu is open, as `.chip:hover` and
            // `.chip[aria-expanded]` do.
            for (state, fill) in [
                ("inactive", widgets.inactive.weak_bg_fill),
                ("active", widgets.active.weak_bg_fill),
            ] {
                assert_eq!(fill, token.face, "{theme}: {state}");
            }
            for (state, fill) in [
                ("hovered", widgets.hovered.weak_bg_fill),
                ("open", widgets.open.weak_bg_fill),
            ] {
                assert_eq!(fill, token.face_2, "{theme}: {state}");
            }
            // Check boxes, radio buttons and slider rails stay findable.
            assert_eq!(widgets.inactive.bg_fill, token.face_2, "{theme}");
            assert_eq!(visuals.text_color(), token.ink, "{theme}");
            assert_eq!(visuals.widgets.inactive.text_color(), token.ink, "{theme}");
            assert_eq!(visuals.weak_text_color(), token.muted, "{theme}");
            assert_eq!(visuals.selection.bg_fill, token.sel, "{theme}");
            assert_eq!(visuals.selection.stroke.color, Color32::WHITE, "{theme}");
            assert_eq!(visuals.hyperlink_color, token.link, "{theme}");
            // Rounded corners and 1 px flat outlines in the mock-up's shadow colour.
            assert_ne!(visuals.window_corner_radius, egui::CornerRadius::ZERO);
            assert_ne!(visuals.menu_corner_radius, egui::CornerRadius::ZERO);
            assert_ne!(
                visuals.widgets.inactive.corner_radius,
                egui::CornerRadius::ZERO
            );
            assert_eq!(visuals.window_stroke, Stroke::new(1.0, token.lo), "{theme}");
            for outline in [
                visuals.widgets.noninteractive.bg_stroke,
                visuals.widgets.inactive.bg_stroke,
            ] {
                assert_eq!(outline, Stroke::new(1.0, token.lo), "{theme}");
            }
        }
    }

    /// WCAG 2.x contrast ratio between two opaque sRGB colours.
    fn contrast(a: Color32, b: Color32) -> f64 {
        let luminance = |c: Color32| {
            let channel = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Every widget state of the modern skin keeps its text and its marks
    /// (check marks, icons, slider grabs, all drawn in the text colour)
    /// legible on its own fills, and plain, weak, strong and link text on
    /// the panels, fields and stripes: at least 4.5:1, the WCAG 2.x AA ratio
    /// for body text. Warnings and errors reach it on fields and stripes,
    /// errors on the face too. A warning on the face takes the mock-ups' own
    /// warn colour instead, which reads 4.23:1 there in the light theme.
    /// A hovered widget also stands apart from a resting one.
    #[test]
    fn every_modern_widget_state_reads_on_its_fill() {
        const AA: f64 = 4.5;
        let mut failures = Vec::new();
        for dark in [false, true] {
            let visuals = skin_visuals(false, dark);
            if visuals.warn_fg_color != mock_up(dark).warn {
                failures.push(format!(
                    "dark {dark}: warning {:?} on the face is not the mock-ups' warn colour",
                    visuals.warn_fg_color
                ));
            }
            let widgets = &visuals.widgets;
            let mut pairs = Vec::new();
            for (state, widget) in [
                ("noninteractive", &widgets.noninteractive),
                ("inactive", &widgets.inactive),
                ("hovered", &widgets.hovered),
                ("active", &widgets.active),
                ("open", &widgets.open),
            ] {
                pairs.push((state, "bg_fill", widget.fg_stroke.color, widget.bg_fill));
                pairs.push((
                    state,
                    "weak_bg_fill",
                    widget.fg_stroke.color,
                    widget.weak_bg_fill,
                ));
            }
            for (ground, fill) in [
                ("panel", visuals.panel_fill),
                ("window", visuals.window_fill),
                ("field", visuals.text_edit_bg_color()),
                ("stripe", visuals.faint_bg_color),
            ] {
                pairs.push(("text", ground, visuals.text_color(), fill));
                pairs.push(("weak text", ground, visuals.weak_text_color(), fill));
                pairs.push(("strong text", ground, visuals.strong_text_color(), fill));
                pairs.push(("link", ground, visuals.hyperlink_color, fill));
                pairs.push(("error", ground, visuals.error_fg_color, fill));
            }
            for (ground, fill) in [
                ("field", visuals.text_edit_bg_color()),
                ("stripe", visuals.faint_bg_color),
            ] {
                pairs.push(("warning", ground, visuals.warn_fg_color, fill));
            }
            pairs.push((
                "selection",
                "bg_fill",
                visuals.selection.stroke.color,
                visuals.selection.bg_fill,
            ));
            for (what, ground, ink, fill) in pairs {
                let ratio = contrast(ink, fill);
                if !ink.is_opaque() || !fill.is_opaque() {
                    failures.push(format!(
                        "dark {dark}: {what} {ink:?} on {ground} {fill:?} is translucent"
                    ));
                } else if ratio < AA {
                    failures.push(format!(
                        "dark {dark}: {what} {ink:?} on {ground} {fill:?} is {ratio:.2}:1"
                    ));
                }
            }
            if widgets.hovered.weak_bg_fill == widgets.inactive.weak_bg_fill {
                failures.push(format!("dark {dark}: hovering a button changes nothing"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    /// Every rectangle a pass painted, nested shapes included.
    fn painted_rects(output: &egui::FullOutput) -> Vec<egui::epaint::RectShape> {
        fn walk(shape: &egui::Shape, out: &mut Vec<egui::epaint::RectShape>) {
            match shape {
                egui::Shape::Rect(rect) => out.push(rect.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => (),
            }
        }
        let mut rects = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut rects);
        }
        rects
    }

    /// A focused text field keeps a visible outline in every skin and theme,
    /// at least 3:1 against the field, the WCAG 2.x ratio for the boundary
    /// of a control. egui outlines it in the selection stroke, which stays
    /// white so that selected glyphs read on the selection fill; the outline
    /// that shows is the last one painted on the field's edge.
    #[test]
    fn a_focused_text_field_keeps_a_visible_outline_in_every_skin() {
        for classic in [false, true] {
            for dark in [false, true] {
                let visuals = skin_visuals(classic, dark);
                let field = visuals.text_edit_bg_color();
                let ctx = egui::Context::default();
                configure_context(&ctx);
                ctx.set_visuals(visuals);
                let id = egui::Id::unique("focused-field");
                let mut text = String::from("text");
                let mut pass = || {
                    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                        ui.add(egui::TextEdit::singleline(&mut text).id(id));
                        outline_focused_field(ui.ctx());
                    });
                    output.textures_delta.clear();
                    output
                };
                pass();
                ctx.memory_mut(|memory| memory.request_focus(id));
                pass();
                let output = pass();
                assert!(ctx.memory(|memory| memory.has_focus(id)));
                let rects = painted_rects(&output);
                let ground = rects
                    .iter()
                    .find(|rect| rect.fill == field)
                    .unwrap_or_else(|| panic!("no field in {rects:?}"))
                    .rect;
                let outline = rects
                    .iter()
                    .rev()
                    .find(|r| {
                        r.rect.min.distance(ground.min) < 0.5
                            && r.rect.max.distance(ground.max) < 0.5
                            && r.stroke.width > 0.0
                    })
                    .unwrap_or_else(|| panic!("no outline in {rects:?}"))
                    .stroke;
                let ratio = contrast(outline.color, field);
                assert!(
                    outline.width >= 1.0 && outline.color.is_opaque() && ratio >= 3.0,
                    "classic {classic}, dark {dark}: focus outline {outline:?} on {field:?} is {ratio:.2}:1"
                );
            }
        }
    }

    /// The modern skin's main-toolbar buttons follow the main-window
    /// mock-up: no frame at rest, and on hover only a 1 px outline in the
    /// shadow colour, without a fill. The classic skin keeps its framed
    /// button, which the toolbar bevels.
    #[test]
    fn modern_toolbar_buttons_show_only_an_outline_on_hover() {
        for classic in [false, true] {
            for dark in [false, true] {
                let visuals = skin_visuals(classic, dark);
                let skin = format!("classic {classic}, dark {dark}");
                let ctx = egui::Context::default();
                configure_context(&ctx);
                ctx.set_visuals(visuals.clone());
                let button = std::cell::Cell::new(egui::Rect::NOTHING);
                let pass = |events: Vec<egui::Event>| {
                    let input = egui::RawInput {
                        events,
                        ..Default::default()
                    };
                    let mut output = ctx.run_ui(input, |ui| {
                        button.set(
                            toolbar_button(
                                ui,
                                classic,
                                "Settings",
                                Some(ui_kit::shell_icons::Icon::Settings),
                            )
                            .rect,
                        );
                    });
                    output.textures_delta.clear();
                    painted_rects(&output)
                };
                pass(Vec::new());
                let rest = pass(Vec::new());
                let frame_of = |rects: &[egui::epaint::RectShape]| {
                    rects
                        .iter()
                        .filter(|r| {
                            r.rect.center().distance(button.get().center()) < 1.0
                                && (r.fill != Color32::TRANSPARENT
                                    || (r.stroke.width > 0.0
                                        && r.stroke.color != Color32::TRANSPARENT))
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                let at_rest = frame_of(&rest);
                let centre = button.get().center();
                pass(vec![egui::Event::PointerMoved(centre)]);
                let hovered = frame_of(&pass(vec![egui::Event::PointerMoved(centre)]));
                if classic {
                    assert!(!at_rest.is_empty(), "{skin}: no frame at rest");
                    continue;
                }
                assert!(at_rest.is_empty(), "{skin}: framed at rest: {at_rest:?}");
                assert_eq!(hovered.len(), 1, "{skin}: {hovered:?}");
                assert_eq!(hovered[0].fill, Color32::TRANSPARENT, "{skin}");
                assert_eq!(
                    hovered[0].stroke,
                    Stroke::new(1.0, mock_up(dark).lo),
                    "{skin}"
                );
            }
        }
    }

    #[test]
    fn the_status_bar_audio_cell_carries_the_speaker() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        configure_context(&ctx);
        // The test machine has no active sound device, so nothing is audible.
        let pass = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        assert!(
            pass.text.iter().any(|(text, _)| text == "🔇 unavailable"),
            "{:?}",
            pass.text
        );
        assert!(pass.elided.is_empty(), "{:?}", pass.elided);
        assert!(shell.stop());
        let pass = status_pass(&ctx, &mut shell, 1040.0, Vec::new());
        assert!(pass.text.iter().any(|(text, _)| text == "🔊 stopped"));
    }

    /// The PC, whose platform opens no files, and the test core, which
    /// opens its scripts: a release once a core that opens files is in.
    static OPENING_CORES: [&dyn unemu_core_api::Core; 2] =
        [&unemu_pc_core::PcCore, &unemu_test_core::TestCore];

    /// The PC alone, whose platform opens no files.
    static PC_ONLY: [&dyn unemu_core_api::Core; 1] = [&unemu_pc_core::PcCore];

    /// The test core twice: two cores answer for one file.
    static TWO_ANSWERS: [&dyn unemu_core_api::Core; 2] =
        [&unemu_test_core::TestCore, &unemu_test_core::TestCore];

    /// A stopped shell whose cores open the test core's scripts, keeping
    /// what machines leave behind under `temp`.
    fn opening_shell(temp: &machine_file::test_support::TempDir) -> Shell {
        let mut shell = shell();
        shell.presenter = None;
        shell.guest_focus = false;
        shell.cores = Arc::new(core_launch::Cores::new(&OPENING_CORES));
        shell.host_dirs = Ok(crate::host_dirs_in(temp.path()).unwrap());
        shell
    }

    /// A script of the test core, its first line written for it.
    fn script(
        temp: &machine_file::test_support::TempDir,
        name: &str,
        lines: &str,
    ) -> std::path::PathBuf {
        let path = temp.path().join(name);
        std::fs::write(&path, format!("unemu-test-core 1\n{lines}\n")).unwrap();
        path
    }

    /// Wait for the worker that prepares the file, and take its answer as a
    /// frame of the shell does.
    fn finish_opening(shell: &mut Shell) {
        let started = Instant::now();
        while shell.opening.is_some() {
            shell.poll_opening();
            assert!(
                started.elapsed() < std::time::Duration::from_secs(60),
                "the file was never prepared"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    fn open(shell: &mut Shell, path: &std::path::Path, ctx: &egui::Context) {
        shell.open_file(path.to_owned(), None, ctx);
        finish_opening(shell);
    }

    #[test]
    fn dash_open_and_file_open_start_a_player() {
        let temp = machine_file::test_support::TempDir::new("shell-open-file");
        let path = script(&temp, "machine.utc", "title Scripted machine");
        let ctx = egui::Context::default();
        // --open FILE --core ID, once the window shows.
        let mut shell = opening_shell(&temp);
        shell.args = crate::parse_args([
            "--open".into(),
            path.clone().into_os_string(),
            "--core".into(),
            "unemu-test-core".into(),
        ])
        .unwrap();
        shell.open_on_start(&ctx);
        finish_opening(&mut shell);
        assert!(shell.player.is_some());
        assert!(shell.presenter.is_none());
        assert_eq!(shell.state(), State::Running);
        assert_eq!(shell.title(), "Scripted machine — Test machine — unemu");
        assert!(shell.stop());
        assert!(shell.player.is_none());

        // File › Open…: above Open machine file…, which has lost Ctrl+O.
        let mut shell = opening_shell(&temp);
        let host = file_dialog::fake::Host::default();
        shell.file_dialogs = file_dialog::FileDialogs::with_host(host.boxed());
        let drawn = draw(&ctx, &mut shell, Vec::new());
        click(&ctx, &mut shell, label_rect(&drawn.text, "File").center());
        let drawn = draw(&ctx, &mut shell, Vec::new());
        let item = label_rect(&drawn.text, "Open…");
        let machine_file = label_rect(&drawn.text, "Open machine file…");
        assert!(item.bottom() <= machine_file.top());
        assert_eq!(
            drawn
                .text
                .iter()
                .filter(|(text, _)| text == "Ctrl+O")
                .count(),
            1
        );
        let key = label_rect(&drawn.text, "Ctrl+O");
        assert!(key.center().y > item.top() && key.center().y < item.bottom());
        assert_eq!(click(&ctx, &mut shell, item.center()), [Command::Open]);
        shell.command(Command::Open, &ctx);
        // The host's dialog opens on everything the cores open.
        let request = host.requests.borrow().last().cloned().unwrap();
        let filters: Vec<&str> = request
            .filters
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            filters,
            [
                "All supported files (*.utc)",
                "unemu test core scripts (*.utc)",
                "All files (*.*)"
            ]
        );
        host.answer(Some(vec![path]));
        shell.poll_file_dialog(&ctx);
        finish_opening(&mut shell);
        assert_eq!(
            shell
                .player
                .as_ref()
                .map(|player| player.prepared().title()),
            Some("Scripted machine")
        );
        assert!(shell.stop());
    }

    #[test]
    fn open_is_greyed_and_says_why_while_no_compiled_in_core_opens_files() {
        let mut shell = shell();
        shell.presenter = None;
        shell.cores = Arc::new(core_launch::Cores::new(&PC_ONLY));
        assert!(!shell.command_allowed(Command::Open));
        // The item explains itself by its hover text, as a pointer finds it.
        let pointer = ui_kit::test_support::Pointer::new(Vec2::new(1040.0, 700.0));
        let mut commands = Vec::new();
        let mut menus = |ui: &mut egui::Ui| shell.menus(ui, &mut commands);
        let shown = pointer.frame(Vec::new(), &mut menus);
        pointer.click(
            ui_kit::test_support::painted_at(&shown, "File").center(),
            &mut menus,
        );
        let shown = pointer.frame(Vec::new(), &mut menus);
        let item = ui_kit::test_support::painted_at(&shown, "Open…").center();
        let hover = pointer.hover(item, &mut menus);
        assert!(shows(&hover, "No core in this release opens files"));
        pointer.click(item, &mut menus);
        assert!(commands.is_empty());
        let ctx = egui::Context::default();
        // Ctrl+O is Open…'s, and asks for no file either.
        shell.command(Command::Open, &ctx);
        assert!(!shell.file_dialogs.is_open());
        // A core that opens files enables it.
        shell.cores = Arc::new(core_launch::Cores::new(&OPENING_CORES));
        assert!(shell.command_allowed(Command::Open));
        assert_eq!(shell.disabled_reason(Command::Open), None);
    }

    /// The release's cores: the NES core opens its cartridge images, so
    /// Open… is enabled and the host's dialog offers them.
    #[test]
    fn open_is_enabled_in_the_release_and_offers_nes_cartridge_images() {
        let mut shell = shell();
        shell.presenter = None;
        let host = file_dialog::fake::Host::default();
        shell.file_dialogs = file_dialog::FileDialogs::with_host(host.boxed());
        assert!(shell.command_allowed(Command::Open));
        assert_eq!(shell.disabled_reason(Command::Open), None);
        shell.command(Command::Open, &egui::Context::default());
        assert!(shell.file_dialogs.is_open());
        let request = host.requests.borrow().last().cloned().unwrap();
        let filters: Vec<&str> = request
            .filters
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            filters,
            [
                "All supported files (*.nes)",
                "NES cartridge images (*.nes)",
                "All files (*.*)"
            ]
        );
    }

    #[test]
    fn a_player_and_the_pc_ask_to_stop_each_other_with_the_stop_prompt() {
        let temp = machine_file::test_support::TempDir::new("shell-open-prompt");
        let path = script(&temp, "machine.utc", "title Scripted machine");
        let ctx = egui::Context::default();
        // Starting the PC while a player runs: the library, then Start, or
        // a machine file.
        for prompt in [StopPrompt::Library, StopPrompt::OpenMachine] {
            let mut shell = opening_shell(&temp);
            open(&mut shell, &path, &ctx);
            assert!(!shell.command_allowed(Command::Start));
            match prompt {
                StopPrompt::Library => shell.command(Command::Library, &ctx),
                _ => {
                    shell.open_path = "picked.toml".into();
                    shell.open_machine();
                }
            }
            assert_eq!(shell.stop_prompt, Some(prompt));
            assert_eq!(shell.session_name(), Some("Scripted machine"));
            assert!(shell.player.is_some());
            shell.answer_stop_prompt(false, &ctx);
            assert!(shell.player.as_ref().unwrap().running());
            shell.command(Command::Library, &ctx);
            shell.answer_stop_prompt(true, &ctx);
            assert!(shell.player.is_none());
            assert_eq!(shell.state(), State::Stopped);
        }
        // Open… while the PC runs: the file is prepared first, then the
        // prompt; Cancel keeps the PC, Stop machine starts the file's.
        let mut shell = opening_shell(&temp);
        shell.presenter = shell_with_presenter();
        open(&mut shell, &path, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::OpenFile));
        assert!(shell.player.is_none());
        shell.answer_stop_prompt(false, &ctx);
        assert!(shell.presenter.is_some());
        assert!(shell.opened.is_none());
        open(&mut shell, &path, &ctx);
        shell.answer_stop_prompt(true, &ctx);
        assert!(shell.presenter.is_none());
        assert_eq!(
            shell
                .player
                .as_ref()
                .map(|player| player.prepared().title()),
            Some("Scripted machine")
        );
        assert!(shell.stop());
    }

    /// The PC of `shell()`, running.
    fn shell_with_presenter() -> Option<Presenter> {
        shell().presenter.take()
    }

    #[test]
    fn pause_reset_power_cycle_and_stop_reach_the_players_machine() {
        let temp = machine_file::test_support::TempDir::new("shell-player-commands");
        let path = script(
            &temp,
            "machine.utc",
            "title Scripted machine\nresets soft,hard\nbattery\nslot 0 tape removable tp Tape deck",
        );
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        open(&mut shell, &path, &ctx);
        // The PC's commands wait; the toolbar keeps its buttons and its
        // Ctrl+Alt+Del button reads Reset.
        for command in [
            Command::Insert(0),
            Command::Layers,
            Command::SaveImage,
            Command::Settings,
        ] {
            assert!(!shell.command_allowed(command), "{command:?}");
        }
        let drawn = draw(&ctx, &mut shell, Vec::new());
        assert!(!shows(&drawn.text, "Ctrl+Alt+Del"));
        for button in ["A:", "B:", "Eject", "Settings", "Layers"] {
            assert!(click(&ctx, &mut shell, label_rect(&drawn.text, button).center()).is_empty());
        }
        let reset = label_rect(&drawn.text, "Reset").center();
        assert_eq!(click(&ctx, &mut shell, reset), [Command::Reset]);
        // The player's Machine menu stands where the PC's does.
        click(
            &ctx,
            &mut shell,
            label_rect(&drawn.text, "Machine").center(),
        );
        let menu = draw(&ctx, &mut shell, Vec::new());
        assert!(shows(
            &menu.text,
            "For the machine: Scripted machine (running)"
        ));
        assert!(!shows(&menu.text, "Disk layers…"));
        egui::Popup::close_all(&ctx);

        shell.command(Command::Pause, &ctx);
        assert!(shell.player.as_ref().unwrap().paused());
        assert_eq!(shell.state(), State::Paused);
        shell.command(Command::Pause, &ctx);
        assert_eq!(shell.state(), State::Running);
        shell.command(Command::Reset, &ctx);
        assert_eq!(shell.player.as_ref().unwrap().message(), "Reset");
        shell.command(Command::PowerCycle, &ctx);
        assert_eq!(shell.player.as_ref().unwrap().message(), "Power cycled");
        let saves = shell
            .player
            .as_ref()
            .unwrap()
            .prepared()
            .saves_dir()
            .unwrap()
            .to_owned();
        shell.command(Command::Stop, &ctx);
        assert!(shell.player.is_none());
        assert_eq!(shell.state(), State::Stopped);
        // Stop finished the machine: its battery counts the start and the
        // power cycle's hard reset.
        let records: Vec<String> = std::fs::read_dir(&saves)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap_or_default())
            .collect();
        assert!(
            records.iter().any(|record| record.ends_with("starts 2\n")),
            "{records:?}"
        );
    }

    #[test]
    fn a_refused_file_shows_its_box_and_leaves_a_running_machine_alone() {
        let temp = machine_file::test_support::TempDir::new("shell-open-refused");
        let notes = temp.path().join("notes.txt");
        std::fs::write(&notes, "hello\n").unwrap();
        let odd = script(&temp, "odd.utc", "unsupported board 5 is not modelled");
        let good = script(&temp, "machine.utc", "title Scripted machine");
        let ctx = egui::Context::default();
        let refused = |shell: &mut Shell, path: &std::path::Path| {
            open(shell, path, &ctx);
            let refusal = shell.open_refusal.take().expect("a refusal");
            assert_eq!(refusal.title, "Open");
            assert_eq!(shell.stop_prompt, None);
            refusal.text
        };
        // While the PC runs.
        let mut shell = opening_shell(&temp);
        shell.presenter = shell_with_presenter();
        assert!(refused(&mut shell, &notes)
            .starts_with("No core in this program opens notes.txt. This program opens: unemu test core scripts (*.utc)"));
        assert!(refused(&mut shell, &odd)
            .contains("recognised odd.utc and cannot run it: board 5 is not modelled"));
        assert!(shell.presenter.as_ref().unwrap().running);
        assert!(shell.player.is_none());
        // While a player runs.
        open(&mut shell, &good, &ctx);
        shell.answer_stop_prompt(true, &ctx);
        refused(&mut shell, &notes);
        assert!(shell.player.as_ref().unwrap().running());
        // Two cores answer: refused, naming them and --core.
        shell.cores = Arc::new(core_launch::Cores::new(&TWO_ANSWERS));
        let several = refused(&mut shell, &good);
        assert!(several.starts_with("More than one core in this program runs machine.utc"));
        assert!(several.contains("--open FILE --core ID"));
        assert!(shell.player.as_ref().unwrap().running());
        // The box shows until OK.
        shell.open_refusal = Some(player_ui::MessageBox::refusal("Open", several));
        assert!(shell.has_dialog());
        assert!(shell.stop());
    }

    #[test]
    fn pad_keys_come_from_the_preferences_and_the_input_dialog_stores_them() {
        use unemu_core_api::{Key, PadControl};
        let temp = machine_file::test_support::TempDir::new("shell-player-pads");
        let path = script(&temp, "pads.utc", "pad 0 a,b Controller 1");
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        let file = temp.path().join("preferences.toml");
        shell.preferences_path = Some(file.clone());
        let mut keys = machine_view::pads::PadBindings::empty();
        keys.bind(0, PadControl::A, Key::SHIFT_RIGHT);
        shell.preferences.set_pad_bindings(&keys);
        open(&mut shell, &path, &ctx);
        assert_eq!(shell.player.as_ref().unwrap().pad_bindings(), &keys);
        // The Input dialog's OK asks the shell to store the player's keys.
        shell.preferences = Default::default();
        shell.player_request(Some(player_ui::Request::PadBindings), &ctx);
        assert_eq!(
            crate::preferences::Preferences::load(&file)
                .unwrap()
                .pad_bindings(),
            keys
        );
        assert!(shell.stop());
    }

    fn player_log(shell: &Shell) -> String {
        String::from_utf8_lossy(&shell.player_log).into_owned()
    }

    #[test]
    fn a_file_prepared_late_never_replaces_a_question_or_an_exit() {
        let temp = machine_file::test_support::TempDir::new("shell-open-late");
        let path = script(&temp, "machine.utc", "title Scripted machine");
        let ctx = egui::Context::default();
        // The library's question stays the library's: Stop machine stops
        // the PC and opens nothing.
        let mut shell = opening_shell(&temp);
        shell.presenter = shell_with_presenter();
        shell.command(Command::Library, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Library));
        open(&mut shell, &path, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Library));
        assert!(shell.opened.is_none());
        shell.answer_stop_prompt(true, &ctx);
        assert!(shell.presenter.is_none());
        assert!(shell.player.is_none());
        // Quitting keeps asking to quit.
        let mut shell = opening_shell(&temp);
        shell.presenter = shell_with_presenter();
        shell.command(Command::Exit, &ctx);
        open(&mut shell, &path, &ctx);
        assert_eq!(shell.stop_prompt, Some(StopPrompt::Exit));
        assert_eq!(shell.exit_progress(), crate::app_menu::ExitProgress::Asking);
        // Stop forgets a file still being prepared, and the status line
        // gets its message back.
        let mut shell = opening_shell(&temp);
        shell.presenter = shell_with_presenter();
        shell.message = "Running".into();
        shell.open_file(path.clone(), None, &ctx);
        assert_eq!(shell.message, "Opening machine.utc…");
        assert!(shell.stop());
        assert!(shell.opening.is_none());
        finish_opening(&mut shell);
        assert!(shell.player.is_none());
        assert_eq!(shell.stop_prompt, None);
    }

    #[test]
    fn a_file_started_while_a_pc_start_is_under_way_stays_the_only_machine() {
        let temp = machine_file::test_support::TempDir::new("shell-open-start");
        let path = script(&temp, "machine.utc", "title Scripted machine");
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        // A machine file waiting for its readiness check to start.
        shell.start_request = Some(0);
        open(&mut shell, &path, &ctx);
        assert!(shell.player.is_some());
        assert_eq!(shell.start_request, None);
        // A PC that arrives anyway is finished, not shown beside it.
        shell.started(shell_with_presenter().unwrap());
        assert!(shell.presenter.is_none());
        assert!(shell.player.is_some());
        assert!(shell.stop());
    }

    #[test]
    fn save_and_load_state_chords_reach_a_player_that_saves_state_and_the_pc_s_guest() {
        let temp = machine_file::test_support::TempDir::new("shell-player-state");
        let path = script(&temp, "state.utc", "saved-state");
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        open(&mut shell, &path, &ctx);
        shell.focus_guest();
        assert!(shell.command_allowed(Command::SaveState));
        let chord = |shell: &mut Shell, key: KeyCode| {
            for key in [KeyCode::ControlLeft, KeyCode::AltLeft, key] {
                shell.key(key, Pressed, false, &ctx);
            }
            for key in [key, KeyCode::AltLeft, KeyCode::ControlLeft] {
                shell.key(key, Released, false, &ctx);
            }
        };
        chord(&mut shell, KeyCode::KeyS);
        let player = shell.player.as_ref().unwrap();
        assert_eq!(player.message(), "State saved");
        let slot = player.prepared().states_dir().unwrap().join("quick.state");
        assert!(slot.is_file());
        assert!(slot.starts_with(temp.path()));
        shell.tick(&ctx, false, std::time::Duration::from_millis(100));
        chord(&mut shell, KeyCode::KeyL);
        assert_eq!(shell.player.as_ref().unwrap().message(), "State loaded");
        assert!(shell.stop());

        // The PC saves no state: both commands stay disabled, and the
        // chords stay the guest's keys.
        let mut shell = keyboard_shell();
        assert!(!shell.command_allowed(Command::SaveState));
        assert!(!shell.command_allowed(Command::LoadState));
        chord(&mut shell, KeyCode::KeyS);
        assert_eq!(
            take_guest_scancodes(&mut shell),
            [0x1D, 0x38, 0x1F, 0x9F, 0xB8, 0x9D]
        );
        chord(&mut shell, KeyCode::KeyL);
        assert_eq!(
            take_guest_scancodes(&mut shell),
            [0x1D, 0x38, 0x26, 0xA6, 0xB8, 0x9D]
        );
    }

    #[test]
    fn host_keys_reach_the_players_pad_and_its_input_dialog_raw() {
        use unemu_core_api::{Key, PadControl};
        let temp = machine_file::test_support::TempDir::new("shell-player-keys");
        let path = script(
            &temp,
            "pads.utc",
            "pad 0 up,down,left,right,a,b,start,select Controller 1\necho-input",
        );
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        open(&mut shell, &path, &ctx);
        shell.focus_guest();
        // X is the first pad's A by default.
        shell.key(KeyCode::KeyX, Pressed, false, &ctx);
        shell.tick(&ctx, false, std::time::Duration::from_millis(50));
        let log = player_log(&shell);
        assert!(log.contains("control: A, pressed: true"), "{log}");
        shell.key(KeyCode::KeyX, Released, false, &ctx);
        // The Input dialog takes the right Shift, which only a raw key
        // event names, for the row that waits.
        shell.player_command(player_ui::Command::Input, &ctx);
        assert!(shell.has_dialog());
        let pointer = ui_kit::test_support::Pointer::new(Vec2::new(1000.0, 800.0));
        let mut requests = Vec::new();
        let mut dialog = |ui: &mut egui::Ui| {
            requests.extend(shell.player.as_mut().unwrap().dialogs(ui.ctx()));
        };
        let mut shown = Vec::new();
        for _ in 0..3 {
            shown = pointer.frame(Vec::new(), &mut dialog);
        }
        pointer.click(
            ui_kit::test_support::painted_at(&shown, "Up").center(),
            &mut dialog,
        );
        drop(dialog);
        shell.key(KeyCode::ShiftRight, Pressed, false, &ctx);
        shell.key(KeyCode::ShiftRight, Released, false, &ctx);
        let mut dialog = |ui: &mut egui::Ui| {
            requests.extend(shell.player.as_mut().unwrap().dialogs(ui.ctx()));
        };
        let shown = pointer.frame(Vec::new(), &mut dialog);
        pointer.click(
            ui_kit::test_support::painted_at(&shown, "OK").center(),
            &mut dialog,
        );
        drop(dialog);
        assert_eq!(requests, [player_ui::Request::PadBindings]);
        assert_eq!(
            shell
                .player
                .as_ref()
                .unwrap()
                .pad_bindings()
                .control(Key::SHIFT_RIGHT),
            Some((0, PadControl::Up))
        );
        assert!(shell.stop());
    }

    #[test]
    fn a_click_on_the_screen_works_an_aimed_players_trigger() {
        let temp = machine_file::test_support::TempDir::new("shell-player-gun");
        let path = script(&temp, "gun.utc", "absolute 0 1 Light gun\necho-input");
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        open(&mut shell, &path, &ctx);
        shell.screen = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(640.0, 480.0));
        shell.pointer = egui::pos2(100.0, 100.0);
        shell
            .events
            .borrow_mut()
            .window(button(winit::event::MouseButton::Left, Pressed));
        shell.input(&ctx, false);
        shell.tick(&ctx, false, std::time::Duration::from_millis(50));
        let log = player_log(&shell);
        assert!(log.contains("PointerButton"), "{log}");
        assert!(log.contains("pressed: true"), "{log}");
        assert!(shell.guest_focus);
        assert!(shell.stop());
    }

    #[test]
    fn a_shell_dialog_holds_the_players_machine() {
        use std::time::Duration;
        let temp = machine_file::test_support::TempDir::new("shell-player-hold");
        let path = script(&temp, "timed.utc", "at 6000 note a tenth of a second in");
        let ctx = egui::Context::default();
        let mut shell = opening_shell(&temp);
        open(&mut shell, &path, &ctx);
        // Ticks a sixtieth of a second apart, as the window paints: the
        // pacing never sees a stall it would skip.
        let mut at = Duration::ZERO;
        let mut run_for = |shell: &mut Shell, ms: u64| {
            let end = at + Duration::from_millis(ms);
            while at < end {
                at = (at + Duration::from_micros(16_667)).min(end);
                shell.tick(&ctx, false, at);
            }
        };
        shell.dialog = Some(("About unemu", String::new()));
        run_for(&mut shell, 400);
        assert!(!player_log(&shell).contains("a tenth"));
        shell.dialog = None;
        run_for(&mut shell, 400);
        assert!(player_log(&shell).contains("a tenth of a second in"));
        assert!(shell.stop());
    }
}
