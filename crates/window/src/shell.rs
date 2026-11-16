// SPDX-License-Identifier: AGPL-3.0-or-later
//! One egui desktop surface around the existing paced machine session.
//!
//! Raw winit events remain intact because guest scan codes describe physical
//! positions, not egui text editing keys. See `design/spec.md` §9.4 and
//! `backlog/docs/design/machine-images.md` §7.

use crate::{
    shell_model::{self, Command, Enablement, LibraryEntry, State},
    Args, Presenter,
};
use egui::{Color32, Sense, Stroke, Vec2};
use std::{
    cell::RefCell, collections::BTreeSet, error::Error, io, rc::Rc, sync::Arc, time::Instant,
};
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

pub(super) fn run(args: Args, autostart: bool) -> Result<(), Box<dyn Error>> {
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
            .with_title("unemupc98")
            .with_icon(crate::logo::icon()?)
            .with_inner_size([1040.0, 700.0])
            .with_min_inner_size([760.0, 500.0]),
        renderer: eframe::Renderer::Glow,
        clipboard_shortcuts: false,
        persist_window: false,
        ..Default::default()
    };
    let app = eframe::create_native(
        "unemupc98",
        options,
        Box::new(move |cc| {
            configure_context(&cc.egui_ctx);
            let mut shell = Shell::new(
                args,
                shell_events,
                shell_errors,
                cc.winit_window().cloned(),
                assembly::firmware_discovery::default_firmware_roots(),
            );
            shell.logo = Some(crate::logo::Logo::load(&cc.egui_ctx)?);
            if autostart {
                shell.start();
            }
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

struct Shell {
    args: Args,
    library: Vec<LibraryEntry>,
    image_library: crate::library_view::LibraryView,
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
    message: String,
    texture: Option<egui::TextureHandle>,
    logo: Option<crate::logo::Logo>,
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
    rom_folder_input: String,
    library_revision: u64,
    readiness: Option<ReadinessJob>,
    prepared: std::collections::BTreeMap<usize, machine_file::PreparedMachineFile>,
    discovery_session: assembly::firmware_discovery::DiscoverySession,
    discovery_roots: Vec<assembly::firmware_discovery::FirmwareRoot>,
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
    discovery_control: assembly::firmware_discovery::DiscoveryControl,
    firmware_cache: Option<std::path::PathBuf>,
    default_store_root: Option<std::path::PathBuf>,
    default_firmware_roots: Vec<assembly::firmware_discovery::FirmwareRoot>,
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
}

/// A shell action that would end a running session, held while the user
/// decides whether to stop the machine for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StopPrompt {
    Library,
    OpenMachine,
    Exit,
}

struct ReadinessJob {
    index: usize,
    revision: u64,
    user_cancelled: bool,
    control: assembly::firmware_discovery::DiscoveryControl,
    receiver: std::sync::mpsc::Receiver<shell_model::PreparedEntry>,
}

impl Shell {
    fn poll_images(&mut self, ctx: &egui::Context) {
        self.image_library.poll(ctx);
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
            self.selected = None;
            let prepared = match transfer.restore() {
                Ok(prepared) => prepared,
                Err(e) => {
                    self.image_library.finish_with_failure(false, true);
                    self.show_error(e.to_string());
                    return;
                }
            };
            let display = display_device(&prepared.machine.devices);
            let result = display
                .ok_or_else(|| "Machine has no display card".to_string())
                .and_then(|display| {
                    let assembled = assembly::assemble_with_verified_bases(
                        &prepared.machine,
                        &assembly::ObservationPolicy {
                            routes: vec![assembly::ObservationRequest {
                                device: display,
                                output: assembly::OutputKind::TextScanout,
                                byte_limit: 0,
                            }],
                            speaker_trace: self.args.speaker_trace,
                            ..Default::default()
                        },
                        self.args.options,
                        &prepared.bases,
                    )
                    .map_err(|e| e.to_string())?;
                    Presenter::from_assembled(
                        &prepared.machine,
                        assembled,
                        display,
                        &mut io::stderr(),
                    )
                    .map_err(|e| e.to_string())
                });
            match result {
                Ok(mut presenter) => {
                    presenter.audio.open(&mut io::stderr());
                    self.started(presenter);
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
            let result = crate::media_dialog::append_entries(
                &mut presenter.assembled,
                &mut presenter.media_config,
                &mut presenter.media_slots,
                draft.unit,
                &entries,
            );
            match result {
                Ok(indices) => presenter
                    .media_events
                    .push((draft.unit, assembly::FloppyAction::Insert(indices[chosen]))),
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
                    let picker = rfd::FileDialog::new().set_title("Choose a floppy image").add_filter("Disk images", &["img", "ima", "dsk"]);
                    let picker = if let Some(window) = &self.window { picker.set_parent(window.as_ref()) } else { picker };
                    if let Some(path) = picker.pick_file() {
                        match crate::media_dialog::browse(&path, draft.slot.geometry) {
                            Ok(entry) => draft.add_browsed(entry),
                            Err(error) => draft.error = Some(error.to_string()),
                        }
                    }
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
                                    let path = crate::media_dialog::resolve_input(directory, &draft.journal_path);
                                    let picker = rfd::FileDialog::new().set_title("Choose a separate journal").set_directory(if draft.journal_path.trim().is_empty() { directory } else { path.parent().unwrap_or(directory) }).set_file_name(crate::media_dialog::filename(&path));
                                    let picker = if let Some(window) = &self.window { picker.set_parent(window.as_ref()) } else { picker };
                                    if let Some(path) = picker.save_file() { draft.journal_path = path.to_string_lossy().into_owned(); }
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
                    let path = crate::media_dialog::resolve_input(&form.directory, &form.output);
                    let picker = rfd::FileDialog::new().set_title("Create blank disk").set_directory(if form.output.trim().is_empty() { form.directory.as_path() } else { path.parent().unwrap_or(&form.directory) }).set_file_name(if form.output.trim().is_empty() { "disk.img".into() } else { crate::media_dialog::filename(&path) });
                    let picker = if let Some(window) = &self.window { picker.set_parent(window.as_ref()) } else { picker };
                    if let Some(path) = picker.save_file() { form.output = path.to_string_lossy().into_owned(); }
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
    }

    fn speed_status(&self) -> String {
        self.presenter
            .as_ref()
            .map_or_else(|| "Speed 100%".into(), |p| p.speed_status())
    }

    fn status(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        let drives = self.drive_views();
        let now = self.elapsed();
        let message = if self.presenter.as_ref().is_some_and(|p| p.mouse.captured()) {
            if self.macos_shortcuts {
                "Pointer captured — ⌘G releases"
            } else {
                "Pointer captured — Ctrl+Alt+G releases"
            }
        } else {
            &self.message
        };
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
                self.drive_chips(ui, cell, &drives, now, commands);
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
        if self.presenter.is_none() {
            if let Some(row) = self.image_library.selected_machine_row() {
                return machine_pending_views(row, |insert| {
                    self.image_library.pending_label(insert)
                });
            }
        }
        match &mut self.presenter {
            Some(presenter) => presenter.drive_views(&origin),
            None => self
                .pending_media
                .as_ref()
                .map_or_else(Vec::new, |pending| pending.drive_views(&origin)),
        }
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
        use crate::media_picker::{CatalogueDisk, Drive, Target, When};
        let words = |write: &assembly::WriteMode, name: &str| match write {
            assembly::WriteMode::ReadOnly => "write-protected".to_owned(),
            assembly::WriteMode::Journal { .. } => format!("changes kept in {name}"),
            assembly::WriteMode::Volatile => "changes lost at Stop".to_owned(),
        };
        let place = |source: &Option<image_store::media_set::CatalogueSource>| {
            let source = source.as_ref()?;
            let set = sets.iter().find(|s| s.image.digest == source.set)?;
            let disk = set.disks.iter().position(|d| d.id == source.medium_id)?;
            Some((source.set.clone(), disk))
        };
        let slot_drives = |slots: &[Option<crate::media_dialog::FloppySlot>; 2],
                           inserted: [Option<usize>; 2],
                           name: &str| {
            (0u8..)
                .zip(slots)
                .filter_map(|(unit, slot)| {
                    let slot = slot.as_ref()?;
                    let now = inserted[usize::from(unit)];
                    let catalogue: Vec<CatalogueDisk> = slot
                        .entries
                        .iter()
                        .filter(|e| e.reason.is_none())
                        .filter_map(|e| {
                            Some(CatalogueDisk {
                                name: if e.medium.name.is_empty() {
                                    "image".into()
                                } else {
                                    e.medium.name.clone()
                                },
                                format: e.geometry.unwrap_or(slot.geometry),
                                write: words(&e.medium.write, name),
                                inserted: e.index.is_some() && e.index == now,
                                index: e.index?,
                                set: place(&e.source),
                            })
                        })
                        .collect();
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
            let inserted =
                [0usize, 1].map(|unit| presenter.fdc().and_then(|fdc| fdc.inserted_medium(unit)));
            let drives = slot_drives(&presenter.media_slots, inserted, &name);
            return (
                vec![Target {
                    id: None,
                    name,
                    when: if halted { When::Halted } else { When::Now },
                    drives,
                    journal_unavailable: None,
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
                !row.drives.is_empty() && row.listing.state != pc_image::machine::State::Running
            }) {
                if row.id() == wanted {
                    at = Some(targets.len());
                }
                let drives = row
                    .drives
                    .iter()
                    .map(|&(unit, geometry)| {
                        let letter = crate::media_picker::drive_name(unit);
                        let pending = row.listing.machine.pending.iter().find(|p| p.drive == letter);
                        Drive {
                            unit,
                            geometry,
                            holds: pending.map_or_else(
                                || "as the image sets it".to_owned(),
                                |p| format!("{} at Start", self.image_library.pending_label(p)),
                            ),
                            catalogue: Vec::new(),
                            current: pending.and_then(|p| {
                                let set = sets.iter().find(|s| s.image.digest == p.set.digest)?;
                                Some((p.set.digest.clone(), set.disks.iter().position(|d| d.id == p.medium)?))
                            }),
                            catalogue_note: Some(
                                "The image's own disks are in the drive at Start; Next disk swaps them once the machine runs.".into(),
                            ),
                        }
                    })
                    .collect();
                targets.push(Target {
                    id: Some(row.id().to_owned()),
                    name: row.name().to_owned(),
                    when: When::AtStart,
                    drives,
                    journal_unavailable: None,
                });
            }
            // The wanted machine is the target or there is none: a disk never
            // goes silently into another machine.
            return match at {
                Some(at) => (targets, at),
                None => (Vec::new(), 0),
            };
        }
        let name = self
            .image_library
            .selected
            .as_ref()
            .map(|row| row.name().to_owned())
            .or_else(|| {
                self.selected
                    .and_then(|i| self.library.get(i))
                    .map(|e| e.name.clone())
            })
            .unwrap_or_else(|| "the machine".into());
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
            }],
            0,
        )
    }

    /// Insert… of drive `unit`: the media picker, on `machine` when given
    /// and with `set` chosen.
    fn open_picker(&mut self, unit: u8, machine: Option<&str>, set: Option<image_store::Digest>) {
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
        self.picker = Some(crate::media_picker::Picker::new(
            targets,
            at,
            unit,
            sets,
            set.as_ref(),
        ));
    }

    fn picker_modal(&mut self, ctx: &egui::Context) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        if let Some(outcome) = crate::media_picker::show(ctx, picker, self.classic) {
            self.apply_picker(outcome);
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
        self.picker = None;
        match outcome {
            Outcome::Cancel => {}
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
                        .push((unit, assembly::FloppyAction::Insert(index)));
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
            Outcome::Browse(unit) => match self.floppy_slot(unit) {
                Some(_) => {
                    let picker = rfd::FileDialog::new()
                        .set_title("Choose a floppy image")
                        .add_filter("Disk images", &["img", "ima", "dsk"]);
                    let picker = if let Some(window) = &self.window {
                        picker.set_parent(window.as_ref())
                    } else {
                        picker
                    };
                    if let Some(path) = picker.pick_file() {
                        self.browse_into(unit, &path);
                    }
                }
                None if self.image_library.machine.is_some() => {
                    self.dialog = Some((
                        "Insert disk",
                        "A disk image file goes into a running machine or a machine file. For a stopped machine, choose a set of this library; Import installation media… adds an image file to it.".into(),
                    ))
                }
                None => {}
            },
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
            egui::Popup::menu(&response).show(|ui| self.drive_menu(ui, view, commands));
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
        commands: &mut Vec<Command>,
    ) {
        ui.add_enabled(false, egui::Label::new(view.menu_label()));
        let Some(unit) = view.floppy_unit() else {
            ui.add_enabled(
                false,
                egui::Button::new((crate::shell_icons::mark(""), view.fixed_eject_label())),
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
            .and_then(|presenter| presenter.fdc())
            .and_then(|fdc| fdc.inserted_medium(usize::from(unit)));
        if let (Some(index), Some(draft)) = (index, &mut self.insert_dialog) {
            let row = crate::drive_chips::entry_position(&draft.slot, index);
            draft.select(row);
        }
    }

    fn new(
        args: Args,
        events: Rc<RefCell<Events>>,
        errors: Rc<RefCell<Option<String>>>,
        window: Option<Arc<Window>>,
        default_firmware_roots: Vec<assembly::firmware_discovery::FirmwareRoot>,
    ) -> Self {
        let root = crate::default_machine()
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default();
        let preferences_path = crate::preferences::config_path().ok();
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
        let (mut paths, mut message) = match shell_model::machine_paths(&root) {
            Ok(paths) => (paths, "Ready".into()),
            Err(error) => (Vec::new(), error.to_string()),
        };
        if let Some(error) = preference_error {
            message = format!("Preferences: {error}");
        }
        let selected = Some(shell_model::select_machine(&mut paths, &args.machine));
        let library = paths.into_iter().map(LibraryEntry::pending).collect();
        let discovery_roots = preferences.roots(&args.rom_dirs, &default_firmware_roots);
        Self {
            args,
            library,
            image_library: crate::library_view::LibraryView::new(
                preferences
                    .store_root
                    .clone()
                    .or_else(|| crate::library_view::default_store_path().ok()),
            ),
            retain_workspace_on_stop: false,
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
            firmware_cache: assembly::firmware_discovery::firmware_cache_path(),
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
        }
    }

    fn state(&self) -> State {
        if self.presenter.is_none() {
            State::Stopped
        } else if self.paused {
            State::Paused
        } else {
            State::Running
        }
    }

    fn command_allowed(&self, command: Command) -> bool {
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
                Command::NewFromImage | Command::ImageSettings => {
                    self.presenter.is_none()
                        && self.image_library.selected.is_some()
                        && !self.image_library.busy
                }
                Command::ImageLayers => self.image_library.selected.is_some(),
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
                    self.image_library.selected.is_some() || self.image_library.active.is_some()
                }
                // A transfer runs on the library worker, which serves the
                // running machine's session until it stops.
                Command::Push | Command::Pull | Command::GetTemplates => {
                    self.presenter.is_none() && self.image_library.active.is_none()
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
                            })
                }
                _ => true,
            }
    }

    /// Tools › Preferences… on its Registries page. Log in… of Get templates…
    /// opens it on that registry's Log in dialog, or on Add registry… for it
    /// when it has no entry: never on another registry's row.
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
    /// New machine wizard, and Preferences for a Log in… of Get templates….
    fn library_commands(&mut self, commands: &mut Vec<Command>) {
        if std::mem::take(&mut self.image_library.firmware_import_wanted) {
            commands.push(Command::ImportFirmware);
        }
        if let Some((machine, unit, set)) = self.image_library.picker_wanted.take() {
            self.open_picker(unit, Some(&machine), Some(set));
        }
        if self.image_library.login_wanted.is_some() {
            commands.push(Command::Preferences);
        }
    }

    /// Get templates… lists again after a Log in… that opened Preferences.
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
            .is_some_and(|presenter| presenter.mouse.captured());
        events.window = self.window.clone();
    }

    fn release_pointer(&mut self) {
        if let Some(presenter) = &mut self.presenter {
            presenter.release_pointer();
        }
        self.sync_routing();
    }

    fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        if self.host_modifiers.super_key() && !modifiers.super_key() {
            let elapsed = self.elapsed();
            if let Some(presenter) = &mut self.presenter {
                presenter.hold_fast_forward(false, elapsed);
            }
            self.fast_forward_key = None;
        }
        // Guest focus resets must not erase modifiers still held by the host.
        self.host_modifiers = modifiers;
    }

    fn reset_keyboard(&mut self) {
        let elapsed = self.elapsed();
        if let Some(presenter) = &mut self.presenter {
            presenter.hold_fast_forward(false, elapsed);
            presenter.focus_changed(false);
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
            && self
                .presenter
                .as_ref()
                .is_some_and(|presenter| presenter.running && presenter.error.is_none())
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
            .preferences_path
            .as_ref()
            .ok_or_else(|| io::Error::other("platform config directory is unavailable"))
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
                    receiver: shell_model::spawn_library_check(
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
                    ),
                });
            }
        }
        if self.readiness.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn started(&mut self, mut presenter: Presenter) {
        self.reset_keyboard();
        self.session = self.session.wrapping_add(1);
        self.events.borrow_mut().session = self.session;
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

    fn save_preferences(&mut self) {
        let result = self
            .preferences_path
            .as_ref()
            .ok_or_else(|| io::Error::other("platform config directory is unavailable"))
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

    fn show_error(&mut self, error: String) {
        self.insert_dialog = None;
        self.blank_disk = None;
        self.stop_prompt = None;
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
        let mut failure = None;
        let had_session = self.presenter.is_some();
        if let Some(mut presenter) = self.presenter.take() {
            if (presenter.running || presenter.halted) && presenter.error.is_none() {
                if let Err(error) = presenter.finish_persistence(&mut io::stderr()) {
                    failure = Some(error.to_string());
                }
            }
            if let Some(error) = &presenter.error {
                failure.get_or_insert_with(|| error.to_string());
            }
            presenter.write_reset_summary(&mut io::stderr());
        }
        let retain_workspace = std::mem::take(&mut self.retain_workspace_on_stop);
        if had_session {
            self.image_library
                .finish_with_failure(retain_workspace, failure.is_some());
        }
        self.texture = None;
        self.paused = false;
        self.modal_paused = false;
        self.message = "Ready".into();
        if had_session {
            self.refresh_library();
        }
        self.sync_routing();
        if let Some(error) = failure {
            self.show_error(error);
            false
        } else {
            if had_session {
                *self.errors.borrow_mut() = None;
                self.dialog = None;
            }
            true
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
        !disposable
            && self
                .presenter
                .as_ref()
                .is_some_and(|presenter| presenter.running && presenter.error.is_none())
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
                    .is_some_and(|presenter| presenter.mouse.captured()),
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
        if !stop {
            self.restore_input = Some(self.stop_prompt_input);
            return;
        }
        match action {
            StopPrompt::Library => {
                self.stop();
            }
            StopPrompt::OpenMachine => self.open_machine_path(),
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
                gradient_title(ui, "unemupc98");
            } else {
                ui.heading("unemupc98");
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
            "unemupc98 is running {}. Stop it and continue?",
            self.session_name().unwrap_or("a machine")
        );
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("stop-running-machine")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            if self.classic {
                gradient_title(ui, "unemupc98");
            } else {
                ui.heading("unemupc98");
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
        if self.presenter.is_none() {
            return;
        }
        if focus {
            self.focus_guest();
        }
        if captured {
            if let Some(presenter) = &mut self.presenter {
                presenter.recapture_pointer();
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
        let elapsed = self.elapsed();
        use Command::*;
        match command {
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
                let picker = rfd::FileDialog::new().set_title("Import ROM folder");
                let picker = if let Some(window) = &self.window { picker.set_parent(window.as_ref()) } else { picker };
                if let Some(folder) = picker.pick_folder() { self.image_library.begin_firmware_import(folder, self.firmware_cache.clone()); }
            },
            StoreFolder => { self.release(); self.store_path_input = self.image_library.root.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(); self.dialog = Some(("Image store folder", "Local store directory".into())); },
            Pause => { self.release_pointer(); if let Some(presenter) = &mut self.presenter { presenter.pacing.resync(elapsed, presenter.assembled.machine.clock().0); } self.paused = !self.paused; self.message = if self.paused { "Paused" } else { "Running" }.into(); },
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
                if self.presenter.as_ref().is_some_and(Presenter::awaits_reset) {
                    self.reset_halted();
                } else if let Some(presenter) = &mut self.presenter {
                    presenter.keyboard.ctrl_alt_del();
                }
            },
            // One Insert… per drive opens the media picker; it replaces the
            // catalogue dialog and From library….
            Insert(unit) => { self.release(); self.open_picker(unit, None, None); },
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
            Next(unit) | Eject(unit) => if let Some(presenter) = &mut self.presenter {
                presenter.media_events.push((unit, if matches!(command, Next(_)) { assembly::FloppyAction::Next } else { assembly::FloppyAction::Eject }));
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
            About => self.dialog = Some(("About unemupc98", format!("unemupc98 {}\nIBM PC/AT emulator\nAGPL-3.0-or-later\nx86jit · egui / eframe · cpal\n\n{}\n\n{}\n\n{}", env!("CARGO_PKG_VERSION"), window::FONT_NOTICE, window::EGUI_FONT_NOTICE, window::SYMBOL_FONT_NOTICE))),
            Placeholder("Open machine file") => self.open_machine_dialog(),
            Placeholder("New machine") => { self.release(); self.image_library.begin_new_machine(); },
            Placeholder(title) => self.dialog = Some((title, "not yet available".into())),
            FirmwareFolders => { self.preferences_draft = self.preferences.clone(); self.rom_folder_input.clear(); self.dialog = Some(("ROM folders", "Firmware is located by SHA-256. Files are read in place; nothing is copied.".into())); },
            Settings => if self.image_library.selected.is_some() { self.release(); self.image_library.open_settings(); } else { self.open_settings() },
            SaveImage => { self.release(); self.image_library.begin_save_image(); },
            Preferences => self.open_preferences(),
            SaveState => {},
            Push => { self.release(); let accounts = self.registry_accounts(); self.image_library.begin_push(accounts); },
            Pull => { self.release(); let accounts = self.registry_accounts(); self.image_library.begin_pull(accounts); },
            GetTemplates => { self.release(); let accounts = self.registry_accounts(); self.image_library.begin_templates(accounts); },
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
            Some(mark) => egui::Button::new((crate::shell_icons::mark(mark), label)),
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
        let machine = self.image_library.machine.is_some();
        // Delete tag… also takes the selected firmware package.
        let image = self.image_library.selected.is_some()
            || (command == DeleteTag && self.image_library.can_delete_package());
        let running = self.presenter.is_some();
        Some(match command {
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
            _ => return None,
        })
    }

    fn menus(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        use Command::*;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                for (label, shortcut, command) in [
                    ("New machine…", "Ctrl+N", Placeholder("New machine")),
                    ("Get templates…", "", GetTemplates),
                    (
                        "Open machine file…",
                        "Ctrl+O",
                        Placeholder("Open machine file"),
                    ),
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
                self.item(ui, "Push…", "", Push, commands);
                self.item(ui, "Pull…", "", Pull, commands);
            });
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
                let reset = if self.presenter.as_ref().is_some_and(Presenter::awaits_reset) {
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
                            egui::Label::new(
                                "Stopped: a disk inserted now goes in at the next Start.",
                            ),
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
            });
            ui.menu_button("Help", |ui| {
                self.item(ui, "Keyboard shortcuts", "F1", Shortcuts, commands);
                self.item(ui, "About unemupc98", "", About, commands);
            });
        });
    }

    fn toolbar(&self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        ui.horizontal_wrapped(|ui| {
            use crate::shell_icons::Icon;
            // The main-window mock-up sets Ctrl+Alt+Del as code text, without an icon.
            for (label, icon, command) in [
                (
                    if self.presenter.is_none() {
                        self.image_library.start_label()
                    } else {
                        "Start"
                    },
                    Some(Icon::Play),
                    Command::Start,
                ),
                (
                    if self.paused { "Resume" } else { "Pause" },
                    Some(if self.paused { Icon::Play } else { Icon::Pause }),
                    Command::Pause,
                ),
                ("Stop", Some(Icon::Stop), Command::Stop),
                ("Ctrl+Alt+Del", None, Command::Reset),
                ("A:", Some(Icon::Floppy), Command::Insert(0)),
                ("B:", Some(Icon::Floppy), Command::Insert(1)),
                ("Eject", Some(Icon::Eject), Command::Eject(0)),
                ("Settings", Some(Icon::Settings), Command::Settings),
                ("Layers", Some(Icon::Layers), Command::Layers),
            ] {
                let response = ui
                    .add_enabled_ui(self.command_allowed(command), |ui| {
                        crate::shell_icons::button(ui, label, icon)
                    })
                    .inner;
                if self.classic {
                    bevel(ui, response.rect, response.is_pointer_button_down_on());
                }
                if response.clicked() {
                    commands.push(command);
                }
            }
            ui.separator();
            self.speed_selector(ui, commands);
        });
    }

    /// The selection View › Speed checks; a held fast-forward leaves it unchanged.
    fn selected_speed(&self) -> crate::pacing::Speed {
        self.presenter
            .as_ref()
            .map_or(crate::pacing::Speed::Normal, |p| p.pacing.selected())
    }

    /// The toolbar shows the mode in effect, so it reads Unlimited while the
    /// fast-forward chord is held and the selected mode again after release.
    fn toolbar_speed(&self) -> crate::pacing::Speed {
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
        if self.presenter.is_some() {
            match (
                &self.image_library.active_machine,
                &self.image_library.active,
            ) {
                // The machines mock-up names the machine and its image.
                (Some((_, name)), Some((row, _, _))) => {
                    format!("{name} — {} — unemupc98", row.reference)
                }
                _ => format!("{} — unemupc98", self.session_name().unwrap_or("Machine")),
            }
        } else {
            "unemupc98".into()
        }
    }

    fn skin(&self, ctx: &egui::Context) {
        let dark = ctx.global_style().visuals.dark_mode;
        ctx.set_visuals(skin_visuals(self.classic, dark));
    }

    /// The host picker for a firmware package's source archive; the library
    /// checks its signature and hash on the worker, never its name.
    fn pick_firmware_source(&mut self, target: crate::library_view::firmware::SourceTarget) {
        use crate::library_view::firmware::SourceTarget;
        self.release();
        let picker = rfd::FileDialog::new().set_title("Choose the source archive (zip or tar)");
        let picker = match &target {
            SourceTarget::Import(_, folder) => picker.set_directory(folder),
            SourceTarget::Edit => picker,
        };
        let picker = if let Some(window) = &self.window {
            picker.set_parent(window.as_ref())
        } else {
            picker
        };
        let path = picker.pick_file();
        self.image_library.firmware_source_picked(target, path);
    }

    /// The host picker for Import installation media…; the worker lists the result.
    fn pick_media(&mut self, pick: crate::library_view::install_media::Pick) {
        use crate::library_view::install_media::Pick;
        self.release();
        let title = match pick {
            Pick::Files => "Choose installation images",
            Pick::Folder => "Choose a folder of installation images",
            Pick::Extras => "Choose extras to store with the set",
        };
        let picker = rfd::FileDialog::new().set_title(title);
        let picker = if let Some(window) = &self.window {
            picker.set_parent(window.as_ref())
        } else {
            picker
        };
        let paths = match pick {
            Pick::Folder => picker.pick_folder().into_iter().collect(),
            Pick::Files | Pick::Extras => picker.pick_files().unwrap_or_default(),
        };
        self.image_library.media_picked(pick, paths);
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
        commands.extend(
            self.image_library
                .show(ui, &self.library, &mut self.selected),
        );
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
                            crate::SINK_SCALE,
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
                    }
                    continue;
                }
            };
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
                            presenter
                                .pacing
                                .resync(elapsed, presenter.assembled.machine.clock().0);
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
        if let Some(presenter) = &mut self.presenter {
            if state == ElementState::Pressed {
                self.guest_held.insert(key);
            } else if !self.guest_held.remove(&key) {
                return;
            }
            if self.macos_shortcuts {
                // The legacy presenter reserves Ctrl+Alt for media and mouse
                // controls; macOS must pass those chords intact to the guest.
                if presenter.running && presenter.error.is_none() {
                    presenter.keyboard.key_event(key, state);
                }
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
        if self.consumed.contains(&key) {
            if !pressed {
                if self.fast_forward_key == Some(key) {
                    self.fast_forward_key = None;
                    let elapsed = self.elapsed();
                    if let Some(presenter) = &mut self.presenter {
                        presenter.hold_fast_forward(false, elapsed);
                    }
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
            if let Some(frame) = presenter.sink.frame().as_ref() {
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
                self.show_error(error);
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
    }
}

fn firmware_progress(
    ui: &mut egui::Ui,
    progress: &assembly::firmware_discovery::DiscoveryProgress,
    reduced_motion: bool,
) {
    use assembly::firmware_discovery::DiscoveryPhase;
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
            if crate::shell_icons::custom_title_bar(self.classic, cfg!(target_os = "macos")) {
                window_title(ui, &self.title());
            }
            self.menus(ui, &mut commands);
            if self.toolbar {
                ui.separator();
                self.toolbar(ui, &mut commands);
            }
        });
        if self.statusbar {
            egui::Panel::bottom("statusbar").show(ui, |ui| self.status(ui, &mut commands));
        }
        self.screen = egui::Rect::NOTHING;
        if self.presenter.is_some() {
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
        let show_firmware_progress = self.visible_discovery() || self.discovery_finished;
        if let Some((title, text)) = &self.dialog {
            egui::Modal::new(egui::Id::unique("shell-dialog")).show(&ctx, |ui| {
                ui.set_max_width(600.0);
                if self.classic {
                    gradient_title(ui, title);
                } else {
                    ui.heading(*title);
                }
                if *title == "About unemupc98" {
                    if let Some(logo) = &self.logo {
                        logo.about(ui);
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        ui.label(text);
                    });
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
                            let picker = rfd::FileDialog::new().set_title("Locate firmware folder");
                            let picker = if let Some(window) = &self.window {
                                picker.set_parent(window.as_ref())
                            } else {
                                picker
                            };
                            if let Some(path) = picker.pick_folder() {
                                if let Err(error) = self.preferences_draft.add_folder(&path) {
                                    self.message = format!("ROM folder: {error}");
                                }
                            }
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
                    ui.label("UNEMUPC98_USER_ROMS_DIR is searched as well when set.");
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
                    if ui.button("OK").clicked() {
                        close_dialog = true;
                    }
                } else if *title == "Image store folder" {
                    ui.text_edit_singleline(&mut self.store_path_input);
                    if ui
                        .add_enabled(
                            self.presenter.is_none() && !self.image_library.busy,
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
                                        crate::library_view::LibraryView::new(Some(path));
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
                } else if ui.button("OK").clicked() {
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
        self.input(&ctx, menu_open || had_dialog || !commands.is_empty());
        for command in commands {
            self.command(command, &ctx);
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
            events.screen = if blocked || self.presenter.is_none() {
                None
            } else {
                Some(self.screen)
            };
            events.pixels_per_point = ctx.pixels_per_point();
        }
        self.cursor(&ctx);
        self.window_close(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop();
        self.image_library.shutdown();
    }
}

/// One Escape closes one dialog: the first reader consumes the key, so a
/// dialog stacked beneath cannot close on the same press.
pub(crate) fn escape_pressed(input: &mut egui::InputState) -> bool {
    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
}

pub(super) fn configure_context(ctx: &egui::Context) {
    // The shortcut symbols need glyphs beyond the proportional Latin font.
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("Hack".into());
    // Last in both families, so it only supplies the marks no egui font has
    // (✓ ✗ ✕ Ⅱ ⌫); without it they would render as empty boxes.
    fonts.font_data.insert(
        "DejaVu-symbols".into(),
        std::sync::Arc::new(egui::FontData::from_static(window::SYMBOL_FONT)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("DejaVu-symbols".into());
    }
    ctx.set_fonts(fonts);
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
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

/// The status bar's speaker, as the main-window mock-up marks its audio cell:
/// crossed out while no sound reaches the speakers of a running machine.
pub(crate) fn audio_mark(audible: bool) -> &'static str {
    if audible {
        "🔊"
    } else {
        "🔇"
    }
}

/// The eject mark of the Media menu and the drive menus.
pub(crate) const EJECT_MARK: &str = "⏏";

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
                .find(|p| p.drive == letter)?;
            Some(crate::drive_chips::DriveView {
                bay: crate::drive_chips::Bay::Floppy { unit, geometry },
                medium: Some(crate::drive_chips::Medium {
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
                }),
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
/// The visuals of one skin in one theme. Selected rows and items draw their
/// text in the selection stroke colour on the selection fill, so every skin
/// keeps that pair legible.
///
/// Both skins take the mock-ups' field colour for text fields, mini tables
/// and scroll tracks, and their stripe colour for striped rows: egui's dark
/// field is near black and reads as a hole in the pane, and its stripe is
/// all but invisible.
pub(crate) fn skin_visuals(classic: bool, dark: bool) -> egui::Visuals {
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    (visuals.extreme_bg_color, visuals.faint_bg_color) = if dark {
        (Color32::from_rgb(38, 40, 43), Color32::from_rgb(44, 46, 50))
    } else {
        (Color32::WHITE, Color32::from_rgb(243, 241, 234))
    };
    if classic {
        let face = if dark {
            Color32::from_rgb(52, 54, 58)
        } else {
            Color32::from_rgb(214, 211, 203)
        };
        visuals.panel_fill = face;
        visuals.window_fill = face;
        visuals.window_corner_radius = egui::CornerRadius::ZERO;
        visuals.menu_corner_radius = egui::CornerRadius::ZERO;
        for widget in [
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
            &mut visuals.widgets.noninteractive,
        ] {
            widget.corner_radius = egui::CornerRadius::ZERO;
        }
        visuals.widgets.inactive.bg_fill = face;
        visuals.widgets.inactive.weak_bg_fill = face;
        visuals.selection.bg_fill = Color32::from_rgb(34, 70, 138);
        // egui draws selected text in the selection stroke colour; the
        // light theme's dark blue would vanish on the classic navy fill.
        visuals.selection.stroke.color = Color32::WHITE;
    }
    visuals
}

pub(crate) fn bevel(ui: &egui::Ui, rect: egui::Rect, pressed: bool) {
    let light = if ui.visuals().dark_mode {
        Color32::from_gray(92)
    } else {
        Color32::WHITE
    };
    let shadow = if ui.visuals().dark_mode {
        Color32::from_gray(24)
    } else {
        Color32::from_rgb(141, 138, 131)
    };
    let (top, bottom) = if pressed {
        (shadow, light)
    } else {
        (light, shadow)
    };
    let rect = rect.shrink(0.5);
    ui.painter()
        .line_segment([rect.left_bottom(), rect.left_top()], Stroke::new(1.0, top));
    ui.painter()
        .line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, top));
    ui.painter().line_segment(
        [rect.right_top(), rect.right_bottom()],
        Stroke::new(1.0, bottom),
    );
    ui.painter().line_segment(
        [rect.right_bottom(), rect.left_bottom()],
        Stroke::new(1.0, bottom),
    );
}

pub(crate) fn gradient_title(ui: &mut egui::Ui, title: &str) {
    title_strip(ui, title, false);
}

/// The main window's classic title strip: the application icon, as the
/// main-window mock-up's title bar shows it, then the title.
fn window_title(ui: &mut egui::Ui, title: &str) {
    title_strip(ui, title, true);
}

fn title_strip(ui: &mut egui::Ui, title: &str, icon: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 23.0), Sense::hover());
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), Color32::from_rgb(19, 51, 111)),
        (rect.right_top(), Color32::from_rgb(75, 121, 193)),
        (rect.right_bottom(), Color32::from_rgb(75, 121, 193)),
        (rect.left_bottom(), Color32::from_rgb(19, 51, 111)),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
    let mut text_left = 7.0;
    if icon {
        let icon_rect = egui::Rect::from_center_size(
            rect.left_center() + Vec2::new(14.0, 0.0),
            Vec2::splat(16.0),
        );
        crate::shell_icons::paint(
            ui.painter(),
            icon_rect,
            crate::shell_icons::Icon::App,
            Color32::WHITE,
            ui.visuals(),
        );
        text_left = 28.0;
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(text_left, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        Color32::WHITE,
    );
    ui.painter().line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, Color32::WHITE),
    );
}

/// Shown in Preferences when the system-shortcut option is on without the
/// Accessibility privilege.
const ACCESS_NOTE: &str = "Allow unemupc98, or the terminal that started it, in System \
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
            "Ctrl+Alt+End",
            "send guest Ctrl+Alt+Del; reset a halted machine",
        ),
        ("Ctrl+Alt+F", "next A: image (Shift for B:)"),
        ("Ctrl+Alt+E", "eject A: (Shift for B:)"),
        ("Ctrl+Alt+I", "Insert… into A: (Shift for B:)"),
        ("Ctrl+Alt+M", "toggle user mute (also while speed-muted)"),
        ("Ctrl+Alt+G", "release pointer"),
        ("Ctrl+Alt+Enter", "full screen"),
        ("Ctrl+N", "New machine… (makes the image and a machine)"),
        ("Ctrl+Shift+N", "New machine from image…"),
        ("F2", "Rename… (machine)"),
        ("Ctrl+D", "Duplicate… (machine)"),
        ("Delete", "Delete machine… / Delete tag… (image)"),
        ("Ctrl+O", "open machine file"),
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
        }
    }

    #[test]
    fn settings_save_selects_the_child_and_live_sessions_reject_publication() {
        let temp = assembly::test_support::TempDir::new("shell-settings");
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
            codes.extend(
                presenter
                    .keyboard
                    .inject_at_boundary(&mut presenter.assembled),
            );
            if presenter
                .assembled
                .machine
                .bus_mut()
                .port_read(0x64, 1)
                .unwrap()
                & 1
                != 0
            {
                presenter
                    .assembled
                    .machine
                    .bus_mut()
                    .port_read(0x60, 1)
                    .unwrap();
            }
            if presenter.keyboard.pending_events() == 0 {
                return codes;
            }
            let cap = presenter.assembled.machine.virtual_time() + 100_000;
            presenter.assembled.machine.run_bounded(Some(cap)).unwrap();
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
        shell.command(Command::Pull, &ctx);
        assert!(shell.image_library.push.is_none() && shell.image_library.pull.is_none());
        shell.stop();
        let temp = assembly::test_support::TempDir::new("shell-registry-transfer");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, "default_registry = \"nas.example:5000\"\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n").unwrap();
        shell.preferences_path = Some(path);
        shell.command(Command::Pull, &ctx);
        let pull = shell.image_library.pull.as_ref().unwrap();
        assert_eq!(pull.default_host, "nas.example:5000");
        assert!(shell.has_dialog());
        shell.image_library.pull = None;
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

    #[test]
    fn get_templates_opens_with_the_shell_accounts_only_while_no_machine_runs() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        assert!(!shell.command_allowed(Command::GetTemplates));
        shell.command(Command::GetTemplates, &ctx);
        assert!(shell.image_library.templates.is_none());
        shell.stop();
        assert!(shell.command_allowed(Command::GetTemplates));
        let temp = assembly::test_support::TempDir::new("shell-templates");
        let path = temp.path().join("preferences.toml");
        std::fs::write(&path, "default_registry = \"nas.example:5000\"\n").unwrap();
        shell.preferences_path = Some(path);
        shell.command(Command::GetTemplates, &ctx);
        assert!(shell.has_dialog());
        let dialog = shell.image_library.templates.as_ref().unwrap();
        assert_eq!(dialog.default_host, "nas.example:5000");
        // Without a store worker the listing ends at once instead of waiting.
        assert!(
            matches!(&dialog.state, crate::templates::TemplatesState::Failed(f) if f.message.contains("worker stopped")),
            "{:?}",
            dialog.state
        );
    }

    #[test]
    fn log_in_from_get_templates_opens_that_registry_login_and_closing_preferences_lists_again() {
        use crate::{
            registry_transfer::TransferEvent,
            templates::{FailureKind, ListingFailure, TemplatesDialog, TemplatesState},
        };
        let mut shell = shell();
        shell.stop();
        let ctx = egui::Context::default();
        let temp = assembly::test_support::TempDir::new("shell-templates-login");
        let path = temp.path().join("preferences.toml");
        std::fs::write(
            &path,
            "default_registry = \"nas.example:5000\"\n[[registries]]\nhost = \"lab.example\"\n[[registries]]\nhost = \"nas.example:5000\"\nprivate = true\n",
        )
        .unwrap();
        shell.preferences_path = Some(path);
        // The listing ended asking for a login, as the worker reports it.
        let (mut dialog, request) = TemplatesDialog::open(shell.registry_accounts());
        let Some(crate::registry_transfer::TransferRequest::Templates { id, .. }) = request else {
            panic!("the dialog asks for the list")
        };
        dialog.on_event(&TransferEvent::Templates {
            id,
            result: Err(ListingFailure {
                kind: FailureKind::LoginRequired,
                host: "nas.example:5000".into(),
                listed: true,
                message: "nas.example:5000 asks for a login.".into(),
            }),
        });
        shell.image_library.login_wanted = dialog.log_in();
        shell.image_library.templates = Some(dialog);
        // The frame turns the request into the Preferences command.
        let mut commands = Vec::new();
        shell.library_commands(&mut commands);
        assert_eq!(commands, [Command::Preferences]);
        shell.command(Command::Preferences, &ctx);
        assert!(shell.image_library.login_wanted.is_none());
        let page = &shell.preferences_dialog.as_ref().unwrap().registries;
        assert_eq!(page.selected_row().unwrap().entry.host, "nas.example:5000");
        assert_eq!(page.login.as_ref().unwrap().host, "nas.example:5000");
        // Closing Preferences lists again; without a store worker the new
        // listing ends at once.
        shell.close_preferences();
        assert!(shell.preferences_dialog.is_none());
        let state = &shell.image_library.templates.as_ref().unwrap().state;
        assert!(
            matches!(state, TemplatesState::Failed(f) if f.message.contains("worker stopped")),
            "{state:?}"
        );
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
        let temp = assembly::test_support::TempDir::new("shell-templates-add");
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
        shell.presenter.as_mut().unwrap().assembled =
            assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
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

    #[test]
    fn invalid_browse_paths_keep_the_session_running_and_the_dialog_editable() {
        let ctx = egui::Context::default();
        for failure in [
            "base alias",
            "missing parent",
            "changed pin",
            "journal alias",
        ] {
            let temp = assembly::test_support::TempDir::new("browse-validation");
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
                    .assembled
                    .floppy_name(assembly::DeviceId::Fdc, 0),
                "first.img"
            );
            assert_eq!(
                shell
                    .presenter
                    .as_ref()
                    .unwrap()
                    .assembled
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
                    [(0, assembly::FloppyAction::Insert(2))]
                );
            }
            assert!(shell.stop());
        }
    }

    #[test]
    fn create_blank_disk_prefills_the_selected_failed_drives_path_and_geometry() {
        let temp = assembly::test_support::TempDir::new("shell-blank-prefill");
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
        let temp = assembly::test_support::TempDir::new("shell-journal-error");
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
                .assembled
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
                .assembled
                .floppy_name(assembly::DeviceId::Fdc, 0),
            "first.img"
        );
        shell.command(Command::InsertFile(0), &ctx);
        shell.insert_dialog.as_mut().unwrap().selected = Some(1);
        shell.confirm_insert();
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "first.img"
        );
        assert_eq!(
            presenter.media_events,
            [(0, assembly::FloppyAction::Insert(1))]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        shell.command(Command::Eject(0), &ctx);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "second.img"
        );
        assert_eq!(presenter.media_events, [(0, assembly::FloppyAction::Eject)]);
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "empty"
        );
    }

    #[test]
    fn browse_insertion_runs_while_paused_and_a_changed_pin_stays_in_the_dialog() {
        let ctx = egui::Context::default();
        for changed in [false, true] {
            let temp = assembly::test_support::TempDir::new("shell-browse");
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
                .assembled
                .machine
                .virtual_time();
            shell.tick(&ctx, false, std::time::Duration::ZERO);
            assert_eq!(
                shell
                    .presenter
                    .as_ref()
                    .unwrap()
                    .assembled
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
                        .assembled
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
                        .flat_map(|state| window::set1_scancodes(shift, state).unwrap())
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
            (KeyO, false, Command::Placeholder("Open machine file")),
            (Comma, false, Command::Settings),
            (Slash, true, Command::Shortcuts),
            (KeyQ, false, Command::Exit),
            (KeyW, false, Command::Exit),
            (KeyM, false, Command::Minimize),
        ] {
            for command_key in [SuperLeft, SuperRight] {
                let mut shell = media_shell();
                shell.presenter.as_mut().unwrap().assembled =
                    assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
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
                                unit,
                                if matches!(expected, Command::Next(_)) {
                                    assembly::FloppyAction::Next
                                } else {
                                    assembly::FloppyAction::Eject
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
                    window::set1_scancodes(key, Pressed).unwrap()
                );
                shell.key(key, Released, false, &ctx);
                assert_eq!(
                    take_guest_scancodes(&mut shell),
                    window::set1_scancodes(key, Released).unwrap()
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
        first.extend(window::set1_scancodes(ArrowLeft, Pressed).unwrap());
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
        let arrow_break = window::set1_scancodes(ArrowLeft, Released).unwrap();
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
    fn a_failed_save_still_asks_for_access_and_reports_both() {
        let (mut shell, log) = system_keys_shell();
        shell.preferences_path = None;
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
        assert!(note.starts_with("Cannot save preferences:"), "{note}");
        assert!(note.ends_with(ACCESS_NOTE), "{note}");
    }

    #[test]
    fn turning_the_option_on_asks_for_accessibility_once_and_saves_it() {
        let temp = assembly::test_support::TempDir::new("shell-system-shortcuts");
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
        shell.presenter.as_mut().unwrap().assembled =
            assembly::assemble(&assembly::at_keyboard(vec![0xEB, 0xFE])).unwrap();
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
            1
        );
        shell.release();
        let pending = shell.presenter.as_ref().unwrap().keyboard.pending_events();
        assert_eq!(pending, 2);
        shell.key(KeyCode::KeyB, Pressed, true, &ctx);
        shell.key(KeyCode::KeyB, Released, true, &ctx);
        shell.key(KeyCode::KeyA, Released, false, &ctx);
        assert_eq!(
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
        let delivered = presenter.mouse.inject_at_boundary(&mut presenter.assembled);
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
            4
        );
        shell.command(Command::Pause, &ctx);
        shell.key(KeyCode::KeyB, Pressed, false, &ctx);
        assert_eq!(
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
                    .assembled
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
                .assembled
                .machine
                .virtual_time(),
            0
        );
        shell.visibility(&ctx, false);
        shell.tick(&ctx, false, Duration::from_millis(200));
        let presenter = shell.presenter.as_ref().unwrap();
        assert!(presenter.occluded_since.is_none());
        assert_eq!(presenter.pacing.dropped, 0);
        assert_eq!(presenter.assembled.machine.virtual_time(), 0);
        shell.tick(&ctx, false, Duration::from_millis(220));
        assert!(
            shell
                .presenter
                .as_ref()
                .unwrap()
                .assembled
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
            1
        );
    }

    fn journal_shell(temp: &assembly::test_support::TempDir, program: Vec<u8>) -> Shell {
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
        shell.presenter = Some(presenter);
        shell
    }

    #[test]
    fn stop_preserves_unsealed_execution_error_journals() {
        let temp = assembly::test_support::TempDir::new("shell-execution-error");
        let mut shell = journal_shell(&temp, vec![0x0f, 0x05]);
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
        assert_eq!(
            std::fs::metadata(temp.path().join("disk.upfj"))
                .unwrap()
                .len(),
            128
        );
    }

    #[test]
    fn stop_finishes_the_session_a_halt_left_open() {
        let temp = assembly::test_support::TempDir::new("shell-finished-halt");
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
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "two.img"
        );
        assert!(presenter.halted);
        let time = presenter.assembled.machine.virtual_time();
        shell.command(Command::Reset, &ctx);
        let presenter = shell.presenter.as_ref().unwrap();
        // The reset button, not keystrokes the halted guest cannot read.
        assert_eq!(presenter.keyboard.pending_events(), 0);
        assert!(presenter.running);
        assert_eq!(presenter.cpu_resets, 1);
        assert_eq!(presenter.assembled.machine.virtual_time(), time);
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
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
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
        temp: &assembly::test_support::TempDir,
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
        let temp = assembly::test_support::TempDir::new("shell-pending-floppy");
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
        assert_eq!(presenter.assembled.machine.virtual_time(), 0);
        assert_eq!(
            presenter.media_events,
            [(0, assembly::FloppyAction::Insert(0))]
        );
        shell.tick(&ctx, false, std::time::Duration::ZERO);
        let presenter = shell.presenter.as_mut().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "boot.img"
        );
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 1),
            "empty"
        );
        assert_eq!(std::fs::read(&path).unwrap(), machine_file);
        assert!(shell.stop());
    }

    #[test]
    fn another_selection_drops_the_floppies_inserted_for_start() {
        let temp = assembly::test_support::TempDir::new("shell-pending-selection");
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
            [(0, assembly::FloppyAction::Insert(2))]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "extra.img"
        );
        assert!(shell.stop());
        shell.image_library.shutdown();
    }

    #[test]
    fn an_error_dialog_on_a_halted_session_still_lets_stop_finish_it() {
        let temp = assembly::test_support::TempDir::new("shell-halt-error");
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
        let temp = assembly::test_support::TempDir::new("shell-halt-exit");
        let base = temp.path().join("cmos.bin");
        std::fs::write(&base, [0; 64]).unwrap();
        let cli = temp.path().join("cli.upfj");
        let mut reference =
            assembly::assemble(&crate::tests::cmos_halt_config(&base, &cli)).unwrap();
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
        assert_eq!(presenter.cpu_resets, 1);
        for key in [KeyCode::End, KeyCode::AltLeft, KeyCode::ControlLeft] {
            shell.key(key, Released, false, &ctx);
        }
        // Neither the chord nor the Shift held at the halt reaches the new
        // boot, not even as the break a later focus loss sends for held keys.
        let presenter = shell.presenter.as_mut().unwrap();
        presenter.focus_changed(false);
        assert_eq!(presenter.keyboard.pending_events(), 0);
        assert!(shell.stop());
    }

    #[test]
    fn start_leaves_out_a_changed_pending_floppy_and_inserts_the_others() {
        let temp = assembly::test_support::TempDir::new("shell-pending-failure");
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
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "empty"
        );
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 1),
            "b.img"
        );
        assert!(shell.stop());
    }

    #[test]
    fn a_changed_machine_file_never_swaps_the_disk_inserted_before_start() {
        let temp = assembly::test_support::TempDir::new("shell-pending-changed");
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
        let temp = assembly::test_support::TempDir::new("shell-pending-edited");
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
                .assembled
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
        temp: &assembly::test_support::TempDir,
        no_boot: bool,
    ) -> Option<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bios = root.join("vendor/unemu-at-bios/bios-unemu-at.bin");
        let floppy = root.join("vendor/freedos/freedos-1200.img");
        for (path, fetch) in [
            (&bios, "vendor/unemu-at-bios/fetch.sh"),
            (&floppy, "vendor/freedos/fetch.sh"),
        ] {
            if !path.exists() {
                let message = format!("missing fixture {} — run {fetch}", path.display());
                assert!(
                    std::env::var_os("UNEMUPC98_REQUIRE_FIXTURES").as_deref()
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
        let capture = presenter.assembled.capture_text(presenter.display).unwrap();
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
        let temp = assembly::test_support::TempDir::new("shell-no-rom-basic");
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
        let resets = shell.presenter.as_ref().unwrap().cpu_resets;
        shell.command(Command::Reset, &ctx);
        let presenter = shell.presenter.as_mut().unwrap();
        assert!(presenter.running);
        assert_eq!(presenter.cpu_resets, resets + 1);
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
            "freedos-1200.img"
        );
        tick_until_screen_shows(&mut shell, &mut elapsed, "FreeDOS kernel", |_| true);
        assert!(shell.presenter.as_ref().unwrap().error.is_none());
        assert!(shell.stop());
    }

    #[test]
    #[ignore = "slow: open AT BIOS POST and a FreeDOS 1.2M boot through the shell"]
    fn a_floppy_inserted_before_start_boots_freedos_on_the_open_bios() {
        let temp = assembly::test_support::TempDir::new("shell-pending-boot");
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
        let temp = assembly::test_support::TempDir::new("shell-failed-finish");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled.persistence[0].fail_output();
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
    fn embedded_font_credits_reach_about_and_informational_output() {
        let mut shell = shell();
        let ctx = egui::Context::default();
        shell.command(Command::About, &ctx);
        let text = &shell.dialog.as_ref().unwrap().1;
        assert!(text.contains(window::FONT_NOTICE));
        assert!(text.contains(window::EGUI_FONT_NOTICE));
        assert!(text.contains(window::SYMBOL_FONT_NOTICE));
        for option in ["--help", "--about", "--version"] {
            let output = crate::informational_output(std::ffi::OsStr::new(option)).unwrap();
            assert!(output.contains(window::EGUI_FONT_NOTICE));
            assert!(output.contains(window::SYMBOL_FONT_NOTICE));
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
            shell.presenter.as_ref().unwrap().keyboard.pending_events(),
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
        let temp = assembly::test_support::TempDir::new("shell-exit-finish-error");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled.persistence[0].fail_output();
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

    fn discovery_shell(temp: &assembly::test_support::TempDir) -> Shell {
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
    ) -> assembly::firmware_discovery::DiscoveryProgress {
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
        let temp = assembly::test_support::TempDir::new("readiness-store-root");
        let root = temp.path().join("configured");
        let store = image_store::Store::open(&root).unwrap();
        let bytes = vec![0xf4; 32768];
        let path = temp.path().join("source.bin");
        std::fs::write(&path, &bytes).unwrap();
        let digest = image_store::Digest::of(&bytes);
        let config = image_store::firmware::Config::decode(format!("schema=1\nplatform='ibm-pc-at'\nname='Synthetic readiness'\n[[roms]]\nid='system-bios'\nrole='system-bios'\nsize=32768\nsha256='{}'\nblob='{}'\n",digest.hex(),digest).as_bytes()).unwrap();
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
                shell.prepared[&0].firmware_sources(),
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
        shell.logo = Some(crate::logo::Logo::load(&ctx).unwrap());
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
        assert!(!texts.iter().any(|t| t == "unemupc98"), "{texts:?}");
    }

    #[test]
    fn checked_start_has_zero_discovery_reads_without_an_error_hint() {
        let temp = assembly::test_support::TempDir::new("checked-start-reuse");
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
        let temp = assembly::test_support::TempDir::new("changed-start-rom");
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
        let temp = assembly::test_support::TempDir::new("failed-start-retry");
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
        let temp = assembly::test_support::TempDir::new("folder-add-scan");
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
        let temp = assembly::test_support::TempDir::new("moved-session-rom");
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
        let temp = assembly::test_support::TempDir::new("incremental-rom-folders");
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
        let temp = assembly::test_support::TempDir::new("running-rom-folders");
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
            let temp = assembly::test_support::TempDir::new("abandoned-start-status");
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
        let temp = assembly::test_support::TempDir::new("changed-machine-start");
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
        use assembly::firmware_discovery::{FirmwareRoot, RootOrigin};
        let temp = assembly::test_support::TempDir::new("isolated-shell-discovery");
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
        assert!(entry.discovery_warnings[0].starts_with("<UNEMUPC98_USER_ROMS_DIR>: skipped"));
    }

    #[test]
    fn cancel_and_rescan_preserve_dialog_disable_start_and_discard_old_readiness() {
        let temp = assembly::test_support::TempDir::new("cancel-rescan-readiness");
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
        let temp = assembly::test_support::TempDir::new("close-firmware-folders");
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
        let temp = assembly::test_support::TempDir::new("superseded-firmware-check");
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
        use assembly::firmware_discovery::{DiscoveryPhase, DiscoveryProgress};
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
        let temp = assembly::test_support::TempDir::new("cancel-queued-rescan");
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
        use assembly::firmware_discovery::{DiscoveryPhase, DiscoveryProgress};
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
        let temp = assembly::test_support::TempDir::new("selected-firmware-readiness");
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
        let temp = assembly::test_support::TempDir::new("shell-rom-readiness");
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
            .select_set(&cache, "example-adventure:1.0", &mut shell.selected);
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
            .assembled
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
        let temp = assembly::test_support::TempDir::new("stop-prompt-name");
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
            "unemupc98 is running DOS games. Stop it and continue?"
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
        let temp = assembly::test_support::TempDir::new("insert-write-policy");
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
            [(0, assembly::FloppyAction::Insert(2))]
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
            [(0, assembly::FloppyAction::Insert(3))]
        );
        presenter.apply_media_events().unwrap();
        assert_eq!(
            presenter.assembled.floppy_name(assembly::DeviceId::Fdc, 0),
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
        let labels = output
            .platform_output
            .accesskit_update
            .take()
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .filter_map(|(_, node)| node.label().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        output.drop_without_applying_deltas();
        StatusPass {
            text,
            fills,
            labels,
            commands,
            elided,
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
            .assembled
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
            .assembled
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
        let temp = assembly::test_support::TempDir::new("chip-from-library");
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

    #[test]
    fn a_set_chosen_in_the_picker_loads_into_the_running_drive_in_order() {
        let temp = assembly::test_support::TempDir::new("library-set-insert");
        let root = temp.path().canonicalize().unwrap();
        let store = root.join("store");
        let mut command: Vec<std::ffi::OsString> =
            vec!["image".into(), "media".into(), "import".into()];
        for (name, byte) in [("one.img", 1u8), ("two.img", 2)] {
            std::fs::write(root.join(name), vec![byte; 1_474_560]).unwrap();
            command.push(root.join(name).into_os_string());
        }
        for arg in [
            "--title",
            "Synthetic install",
            "--tag",
            "test:set",
            "--label",
            "1=Setup",
            "--label",
            "2=Data",
            "--create",
            "--store",
        ] {
            command.push(arg.into());
        }
        command.push(store.clone().into_os_string());
        let mut errors = Vec::new();
        assert_eq!(
            cli::command::run(&command, &mut io::sink(), &mut errors),
            0,
            "{}",
            String::from_utf8_lossy(&errors)
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
            .select_set(&cache, "test:set", &mut shell.selected);
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
        assert_eq!(picker.sets[chosen].reference, "test:set");
        let outcome = picker.insert().expect("the chosen set goes in");
        assert!(matches!(
            &outcome,
            crate::media_picker::Outcome::Set { unit: 0, reference, first: 0, all: true, journal: true, .. }
                if reference == "test:set"
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
        let chosen = library.rows.iter().position(|row| row.tag == "test:set");
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
            [(0, assembly::FloppyAction::Insert(2))]
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
            [(0, assembly::FloppyAction::Eject)]
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
        let temp = assembly::test_support::TempDir::new("chip-write-mode");
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
        /// Selectable items and whether each is marked as the current choice.
        toggled: Vec<(String, bool)>,
        commands: Vec<Command>,
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
        let toggled = output
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
                            node.toggled()? == egui::accesskit::Toggled::True,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        output.drop_without_applying_deltas();
        Drawn {
            text,
            toggled,
            commands,
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
        let temp = assembly::test_support::TempDir::new("exit-through-dialogs");
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
            Some("About unemupc98")
        );
        assert!(commands.contains(&egui::ViewportCommand::Minimized(false)));
        assert!(commands.contains(&egui::ViewportCommand::Focus));
        shell.minimized = false;
        // An open dialog holds back everything but quitting.
        native(&shell, NativeRequest::Command(Command::Shortcuts));
        let commands = quit_pass(&mut shell, &ctx);
        assert_eq!(
            shell.dialog.as_ref().map(|(title, _)| *title),
            Some("About unemupc98")
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
        let temp = assembly::test_support::TempDir::new("shell-system-quit-stop");
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
        let temp = assembly::test_support::TempDir::new("shell-system-quit-failed-stop");
        let mut shell = journal_shell(&temp, vec![0xeb, 0xfe]);
        shell.presenter.as_mut().unwrap().assembled.persistence[0].fail_output();
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
        let temp = assembly::test_support::TempDir::new("refused-insert");
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
        let temp = assembly::test_support::TempDir::new("picker-browse");
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
            [(0, assembly::FloppyAction::Insert(1))]
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
            reference: "disks:set".into(),
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
                .position(|row| row.tag == "disks:set")
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
            presenter
                .media_events
                .push((0, assembly::FloppyAction::Eject));
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
                    window_title(ui, "unemupc98");
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
}
