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
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Default)]
struct Events {
    queue: Vec<HostEvent>,
    guest_focus: bool,
    captured: bool,
    window: Option<Arc<Window>>,
    screen: Option<egui::Rect>,
    pointer: egui::Pos2,
    pixels_per_point: f32,
    session: u64,
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
    state: ElementState,
    repeat: bool,
    synthetic: bool,
    guest_focus: bool,
    session: u64,
}

impl Events {
    #[cfg(test)]
    fn key(&mut self, key: KeyCode, state: ElementState) -> bool {
        self.keyboard(key, state, false, false)
    }

    fn keyboard(
        &mut self,
        key: KeyCode,
        state: ElementState,
        repeat: bool,
        synthetic: bool,
    ) -> bool {
        self.queue.push(HostEvent::Key(HostKey {
            key,
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
                self.keyboard(key, event.state, event.repeat, *is_synthetic)
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
    let event_loop = EventLoop::<eframe::UserEvent>::with_user_event().build()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("unemupc98")
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
            let mut shell =
                Shell::new(args, shell_events, shell_errors, cc.winit_window().cloned());
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
    events: Rc<RefCell<Events>>,
    errors: Rc<RefCell<Option<String>>>,
    window: Option<Arc<Window>>,
    screen: egui::Rect,
    guest_focus: bool,
    pointer: egui::Pos2,
    modifiers: [bool; 6],
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
    start_request: Option<usize>,
    checked_selection: Option<usize>,
    insert_dialog: Option<crate::media_dialog::InsertDialog>,
    blank_disk: Option<crate::media_dialog::BlankDisk>,
    discovery_warnings: BTreeSet<String>,
}

struct ReadinessJob {
    index: usize,
    revision: u64,
    receiver: std::sync::mpsc::Receiver<shell_model::PreparedEntry>,
}

impl Shell {
    fn has_dialog(&self) -> bool {
        self.dialog.is_some() || self.insert_dialog.is_some() || self.blank_disk.is_some()
    }

    fn confirm_insert(&mut self) {
        let Some(draft) = &self.insert_dialog else {
            return;
        };
        let entry = match draft.selection() {
            Ok(entry) => entry,
            Err(error) => {
                if let Some(draft) = &mut self.insert_dialog {
                    draft.error = Some(error);
                }
                return;
            }
        };
        if let Some(presenter) = &mut self.presenter {
            let index = if let Some(index) = entry.index {
                index
            } else {
                match presenter.assembled.append_floppy(
                    assembly::DeviceId::Fdc,
                    draft.unit,
                    &mut presenter.media_config,
                    &entry.medium,
                ) {
                    Ok(index) => {
                        if let Some(slot) = &mut presenter.media_slots[usize::from(draft.unit)] {
                            slot.entries.push(crate::media_dialog::CatalogueEntry {
                                index: Some(index),
                                ..entry
                            });
                        }
                        index
                    }
                    Err(error) => {
                        if let Some(draft) = &mut self.insert_dialog {
                            draft.error = Some(error.to_string());
                        }
                        return;
                    }
                }
            };
            presenter
                .media_events
                .push((draft.unit, assembly::FloppyAction::Insert(index)));
        }
        self.insert_dialog = None;
    }

    fn media_modals(&mut self, ctx: &egui::Context) {
        let mut insert = false;
        let mut cancel = false;
        if let Some(draft) = &mut self.insert_dialog {
            let title = format!(
                "Insert floppy into {}:",
                if draft.unit == 0 { "A" } else { "B" }
            );
            egui::Modal::new(egui::Id::unique("insert-floppy")).show(ctx, |ui| {
                ui.set_width(560.0);
                if self.classic { gradient_title(ui, &title); } else { ui.heading(title); }
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
                            Ok(entry) => {
                                let compatible = entry.reason.is_none();
                                draft.slot.entries.push(entry);
                                draft.select(compatible.then_some(draft.slot.entries.len() - 1));
                            }
                            Err(error) => draft.error = Some(error.to_string()),
                        }
                    }
                }
                if let Some(entry) = draft.selected.and_then(|index| draft.slot.entries.get(index)) {
                    if let Some(geometry) = entry.geometry {
                        ui.label(format!("{} · {}/{}/{} · detected from size", crate::media_dialog::geometry_name(geometry), geometry.cylinders, geometry.heads, geometry.sectors_per_track));
                    }
                    ui.label("Write mode");
                    if entry.index.is_some() {
                        ui.label(format!("{} — fixed by the machine file", crate::media_dialog::write_name(&entry.medium.write)));
                        if let assembly::WriteMode::Journal { keep_generations, .. } = entry.medium.write {
                            ui.label(format!("Retains {keep_generations} closed generations; base stays read-only."));
                        }
                    } else {
                        ui.radio_value(&mut draft.journal, false, "Read-only");
                        ui.radio_value(&mut draft.journal, true, "Journal (keep changes separately)");
                        if draft.journal {
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
                if let Some(error) = &draft.error { ui.add(egui::Label::new(error).wrap()); }
                ui.separator();
                ui.horizontal(|ui| {
                    insert = ui.add_enabled(draft.selection().is_ok(), egui::Button::new("Insert")).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                cancel |= ui.input(|input| input.key_pressed(egui::Key::Escape));
            });
        }
        if cancel {
            self.insert_dialog = None;
        } else if insert {
            self.confirm_insert();
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
                close |= ui.input(|input| input.key_pressed(egui::Key::Escape));
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

    fn status(&self, ui: &mut egui::Ui) {
        let message = if self.presenter.as_ref().is_some_and(|p| p.mouse.captured()) {
            "Pointer captured — Ctrl+Alt+G releases"
        } else {
            &self.message
        };
        let entry = self.selected.and_then(|index| self.library.get(index));
        let audio = self.presenter.as_ref().map_or("Audio stopped", |p| {
            if !p.audio.available() {
                "Audio unavailable"
            } else if p.audio.muted() {
                "Audio muted"
            } else {
                "Audio on"
            }
        });
        let labels = [
            message,
            "Power",
            "A:",
            "C:",
            entry
                .filter(|entry| !entry.rate.is_empty())
                .map_or("CPU unknown", |entry| &entry.rate),
            entry.map_or("EMS unknown", |entry| &entry.ems),
            audio,
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
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(cell.shrink(3.0))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.set_clip_rect(cell.intersect(ui.clip_rect()));
                    if (1..=3).contains(&index) {
                        led(ui, index == 1 && self.presenter.is_some(), label);
                        if index > 1 {
                            ui.response().on_hover_text(
                                "Drive activity unavailable at the observation boundary.",
                            );
                        }
                    } else {
                        ui.add(egui::Label::new(label).truncate())
                            .on_hover_text(label);
                    }
                },
            );
            bevel(ui, cell, true);
        }
    }

    fn new(
        args: Args,
        events: Rc<RefCell<Events>>,
        errors: Rc<RefCell<Option<String>>>,
        window: Option<Arc<Window>>,
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
        Self {
            args,
            library,
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
            events,
            errors,
            window,
            screen: egui::Rect::NOTHING,
            guest_focus: false,
            pointer: egui::Pos2::ZERO,
            modifiers: [false; 6],
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
            start_request: None,
            checked_selection: None,
            insert_dialog: None,
            blank_disk: None,
            discovery_warnings: BTreeSet::new(),
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

    fn enablement(&self) -> Enablement {
        let entry = self.selected.and_then(|index| self.library.get(index));
        Enablement {
            state: self.state(),
            ready: entry.is_some_and(|entry| entry.checked && entry.warning.is_none()),
            floppy: self
                .presenter
                .as_ref()
                .filter(|presenter| presenter.running)
                .map_or([false; 2], |presenter| {
                    presenter.media_slots.each_ref().map(Option::is_some)
                }),
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

    fn reset_keyboard(&mut self) {
        if let Some(presenter) = &mut self.presenter {
            presenter.focus_changed(false);
        }
        self.modifiers = [false; 6];
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

    fn start(&mut self) {
        if self.presenter.is_some() {
            return;
        }
        self.reset_keyboard();
        if let Some(index) = self.selected {
            self.start_request = Some(index);
            self.library[index].checked = false;
            self.library[index].warning = Some("Checking firmware…".into());
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
                    presenter.audio.open(&mut io::stderr());
                    self.started(presenter);
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

    fn poll_readiness(&mut self, ctx: &egui::Context) {
        if self.selected != self.checked_selection {
            if let Some(index) = self.selected {
                self.library[index].checked = false;
            }
            self.checked_selection = self.selected;
        }
        if let Some(job) = &self.readiness {
            match job.receiver.try_recv() {
                Ok(prepared) => {
                    let (index, revision) = (job.index, job.revision);
                    self.readiness = None;
                    self.apply_readiness(index, revision, prepared);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    let index = job.index;
                    self.readiness = None;
                    self.library[index].checked = true;
                    self.library[index].warning = Some("Firmware check failed".into());
                    self.start_request = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.presenter.is_none() && self.readiness.is_none() {
            if let Some(index) = self.selected.filter(|index| !self.library[*index].checked) {
                let wake = ctx.clone();
                self.readiness = Some(ReadinessJob {
                    index,
                    revision: self.library_revision,
                    receiver: shell_model::spawn_library_check(
                        self.library[index].path.clone(),
                        self.args.options,
                        self.preferences.roots(&self.args.rom_dirs),
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
        self.paused = false;
        self.guest_focus = true;
        self.modal_paused = false;
        self.texture = None;
        self.message =
            "Running — click the screen to capture the pointer; Ctrl+Alt+G releases".into();
        *self.errors.borrow_mut() = None;
        self.dialog = None;
        self.sync_routing();
    }

    fn refresh_library(&mut self) {
        self.library_revision = self.library_revision.wrapping_add(1);
        self.start_request = None;
        self.discovery_warnings.clear();
        for entry in &mut self.library {
            *entry = LibraryEntry::pending(entry.path.clone());
        }
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
        self.release();
        if let Some(presenter) = &mut self.presenter {
            presenter.running = false;
        }
        *self.errors.borrow_mut() = Some(error.clone());
        self.message = error.clone();
        self.dialog = Some(("Machine error", error));
    }

    fn open_machine(&mut self) {
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
        self.blank_disk = None;
        self.release();
        self.session = self.session.wrapping_add(1);
        self.events.borrow_mut().session = self.session;
        self.start_request = None;
        let mut failure = None;
        let had_session = self.presenter.is_some();
        if let Some(mut presenter) = self.presenter.take() {
            if presenter.running && presenter.error.is_none() {
                if let Err(error) = presenter.finish_persistence(&mut io::stderr()) {
                    failure = Some(error.to_string());
                }
            }
            if let Some(error) = &presenter.error {
                failure.get_or_insert_with(|| error.to_string());
            }
            presenter.write_reset_summary(&mut io::stderr());
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

    fn command(&mut self, command: Command, ctx: &egui::Context) {
        if !self.enablement().allows(command) {
            return;
        }
        use Command::*;
        match command {
            Start => self.start(),
            Pause => { self.release_pointer(); self.paused = !self.paused; self.message = if self.paused { "Paused" } else { "Running" }.into(); },
            Stop | Library => { self.stop(); },
            PowerCycle => { if self.stop() { self.start(); } },
            Reset => { self.guest_held.clear(); if let Some(presenter) = &mut self.presenter {
                presenter.keyboard.ctrl_alt_del();
            } },
            Insert(unit) => if let Some(slot) = self.presenter.as_ref().and_then(|p| p.media_slots[usize::from(unit)].clone()) {
                self.release();
                self.insert_dialog = Some(crate::media_dialog::InsertDialog::new(unit, slot));
            },
            CreateBlank => {
                self.release();
                let entry = self.selected.and_then(|index| self.library.get(index));
                self.blank_disk = Some(crate::media_dialog::BlankDisk::for_machine(entry.map_or(self.args.machine.as_path(), |entry| entry.path.as_path()), entry.and_then(|entry| entry.warning.as_deref())));
            },
            Next(unit) | Eject(unit) => if let Some(presenter) = &mut self.presenter {
                presenter.media_events.push((unit, if matches!(command, Next(_)) { assembly::FloppyAction::Next } else { assembly::FloppyAction::Eject }));
            },
            Mute => if let Some(presenter) = &mut self.presenter { presenter.audio.toggle_mute(); },
            Release => self.release_pointer(),
            Fullscreen => { self.fullscreen = !self.fullscreen; ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen)); },
            Exit => { self.stop(); ctx.send_viewport_cmd(egui::ViewportCommand::Close); },
            Shortcuts => self.dialog = Some(("Keyboard shortcuts", SHORTCUTS.into())),
            About => self.dialog = Some(("About unemupc98", format!("unemupc98 {}\nIBM PC/AT emulator\nAGPL-3.0-or-later\nx86jit · egui / eframe · cpal\n\n{}\n\n{}", env!("CARGO_PKG_VERSION"), window::FONT_NOTICE, window::EGUI_FONT_NOTICE))),
            Placeholder(title) => self.dialog = Some((title, if title == "Open machine file" { "Open a machine description in the library." } else { "not yet available" }.into())),
            FirmwareFolders => { self.preferences_draft = self.preferences.clone(); self.rom_folder_input.clear(); self.dialog = Some(("ROM folders", "Firmware is located by SHA-256. Files are read in place; nothing is copied.".into())); },
            SaveState => {},
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
        if ui
            .add_enabled(
                self.enablement().allows(command),
                egui::Button::new(label).shortcut_text(shortcut),
            )
            .clicked()
        {
            commands.push(command);
            ui.close();
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        use Command::*;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                for (label, shortcut, command) in [
                    ("New machine…", "Ctrl+N", Placeholder("New machine")),
                    (
                        "Open machine file…",
                        "Ctrl+O",
                        Placeholder("Open machine file"),
                    ),
                    ("Import image…", "", Placeholder("Import image")),
                ] {
                    self.item(ui, label, shortcut, command, commands);
                }
                ui.separator();
                self.item(ui, "Machine library", "", Library, commands);
                self.item(ui, "Exit", "", Exit, commands);
            });
            ui.menu_button("Machine", |ui| {
                for (label, shortcut, command) in [
                    ("Start", "Enter", Start),
                    (
                        if self.paused { "Resume" } else { "Pause" },
                        "Ctrl+Alt+P",
                        Pause,
                    ),
                    ("Send Ctrl+Alt+Del", "Ctrl+Alt+End", Reset),
                    ("Power cycle", "", PowerCycle),
                    ("Stop", "Ctrl+Alt+Q", Stop),
                ] {
                    self.item(ui, label, shortcut, command, commands);
                }
                ui.separator();
                self.item(
                    ui,
                    "Save state (not yet available)",
                    "",
                    SaveState,
                    commands,
                );
                ui.separator();
                self.item(ui, "Settings…", "Ctrl+,", Placeholder("Settings"), commands);
                self.item(ui, "Disk layers…", "", Placeholder("Disk layers"), commands);
            });
            ui.menu_button("Media", |ui| {
                for unit in 0..2 {
                    let name = self
                        .presenter
                        .as_mut()
                        .map(|p| p.assembled.floppy_name(assembly::DeviceId::Fdc, unit))
                        .unwrap_or_else(|| "stopped".into());
                    let drive = if unit == 0 { "A:" } else { "B:" };
                    let present = self.enablement().floppy[unit as usize];
                    let geometry = self
                        .presenter
                        .as_ref()
                        .and_then(|p| p.media_slots[usize::from(unit)].as_ref())
                        .map(|slot| crate::media_dialog::geometry_name(slot.geometry));
                    ui.add_enabled(
                        present,
                        egui::Label::new(if let Some(geometry) = geometry {
                            format!("Drive {drive} ({geometry}) — {name}")
                        } else {
                            format!(
                                "Drive {drive} — {}",
                                if self.presenter.is_none() {
                                    "machine stopped"
                                } else {
                                    "not installed"
                                }
                            )
                        }),
                    );
                    ui.add_enabled_ui(present, |ui| {
                        self.item(
                            ui,
                            &format!("Insert into {drive}…"),
                            if unit == 0 { "Ctrl+Alt+I" } else { "" },
                            Insert(unit),
                            commands,
                        );
                    });
                    self.item(
                        ui,
                        "Next image in catalogue",
                        if unit == 0 {
                            "Ctrl+Alt+F"
                        } else {
                            "Ctrl+Alt+Shift+F"
                        },
                        Next(unit),
                        commands,
                    );
                    self.item(
                        ui,
                        &format!("Eject {drive}"),
                        if unit == 0 {
                            "Ctrl+Alt+E"
                        } else {
                            "Ctrl+Alt+Shift+E"
                        },
                        Eject(unit),
                        commands,
                    );
                    ui.separator();
                }
                ui.add_enabled(
                    false,
                    egui::Label::new(
                        self.presenter
                            .as_ref()
                            .map_or("C: — machine stopped", |p| &p.fixed_disk),
                    ),
                );
                self.item(ui, "Disk layers…", "", Placeholder("Disk layers"), commands);
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
                self.item(ui, "Mute audio", "Ctrl+Alt+M", Mute, commands);
                ui.menu_button("Skin", |ui| {
                    ui.selectable_value(&mut self.classic, true, "Classic");
                    ui.selectable_value(&mut self.classic, false, "Modern");
                });
                ui.menu_button("Theme", |ui| {
                    egui::widgets::global_theme_preference_buttons(ui);
                });
            });
            ui.menu_button("Tools", |ui| {
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
                            "Locate firmware" | "Preferences" => FirmwareFolders,
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

    fn title(&self) -> String {
        if self.presenter.is_some() {
            format!(
                "{} — unemupc98",
                self.selected
                    .and_then(|index| self.library.get(index))
                    .map_or("Machine", |entry| entry.name.as_str())
            )
        } else {
            "unemupc98".into()
        }
    }

    fn skin(&self, ctx: &egui::Context) {
        let dark = ctx.global_style().visuals.dark_mode;
        let mut visuals = if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        if self.classic {
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
        }
        ctx.set_visuals(visuals);
    }

    fn welcome(&mut self, ui: &mut egui::Ui, commands: &mut Vec<Command>) {
        egui::Panel::left("welcome-side")
            .exact_size(222.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 60, 116))
                    .inner_margin(18.0),
            )
            .show(ui, |ui| {
                ui.style_mut().visuals.override_text_color = Some(Color32::WHITE);
                ui.add_space(10.0);
                ui.heading("Welcome to\nunemupc98");
                ui.add_space(14.0);
                ui.label("Pick a machine to start it, or set up a new one.");
                ui.add_space(12.0);
                for (label, command) in [
                    ("New machine…", Command::Placeholder("New machine")),
                    (
                        "Open machine file…",
                        Command::Placeholder("Open machine file"),
                    ),
                    ("Import image…", Command::Placeholder("Import image")),
                    ("Keyboard shortcuts", Command::Shortcuts),
                ] {
                    if ui
                        .add_sized([186.0, 28.0], egui::Button::new(label))
                        .clicked()
                    {
                        commands.push(command);
                    }
                }
            });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            ui.columns(2, |columns| {
                columns[0].heading("Machines");
                egui::ScrollArea::both()
                    .id_salt("machines")
                    .show(&mut columns[0], |ui| {
                        egui::Grid::new("library").striped(true).show(ui, |ui| {
                            ui.strong("Name");
                            ui.strong("File");
                            ui.strong("Last run");
                            ui.end_row();
                            for (index, entry) in self.library.iter().enumerate() {
                                let label = format!(
                                    "{}{}",
                                    if entry.has_problem() { "[!] " } else { "" },
                                    entry.name
                                );
                                let response =
                                    ui.selectable_label(self.selected == Some(index), label);
                                if response.clicked() || response.double_clicked() {
                                    if self.selected != Some(index) {
                                        self.library_revision =
                                            self.library_revision.wrapping_add(1);
                                        self.start_request = None;
                                    }
                                    self.selected = Some(index);
                                }
                                if response.double_clicked() {
                                    commands.push(Command::Start);
                                }
                                ui.label(
                                    entry.path.file_name().unwrap_or_default().to_string_lossy(),
                                );
                                ui.label("");
                                ui.end_row();
                            }
                        });
                    });
                columns[1].heading("Details");
                egui::ScrollArea::vertical()
                    .id_salt("details")
                    .show(&mut columns[1], |ui| {
                        if let Some(entry) = self.selected.and_then(|index| self.library.get(index))
                        {
                            ui.strong(&entry.name);
                            ui.add_space(8.0);
                            egui::Grid::new("machine-details").show(ui, |ui| {
                                for (name, value) in &entry.details {
                                    ui.label(name);
                                    ui.label(value);
                                    ui.end_row();
                                }
                            });
                            ui.label(&entry.ems);
                            if let Some(warning) = &entry.warning {
                                ui.add_space(8.0);
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(warning.replace('/', "/\u{200b}"))
                                                .color(if ui.visuals().dark_mode {
                                                    Color32::LIGHT_YELLOW
                                                } else {
                                                    Color32::from_rgb(138, 83, 0)
                                                }),
                                        )
                                        .wrap(),
                                    );
                                    ui.add(egui::Label::new(entry.hint()).wrap());
                                });
                                if entry.hint().contains("Create blank disk")
                                    && ui.button("Create blank disk…").clicked()
                                {
                                    commands.push(Command::CreateBlank);
                                }
                            }
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        self.enablement().allows(Command::Start),
                                        egui::Button::new("Start"),
                                    )
                                    .clicked()
                                {
                                    commands.push(Command::Start);
                                }
                                if ui.button("Settings…").clicked() {
                                    commands.push(Command::Placeholder("Settings"));
                                }
                                if ui.button("Layers…").clicked() {
                                    commands.push(Command::Placeholder("Disk layers"));
                                }
                            });
                        } else {
                            ui.label("No machine files found in machines/ or machines/local/.");
                        }
                    });
            });
        });
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
        let events = std::mem::take(&mut self.events.borrow_mut().queue);
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
                WindowEvent::Focused(false) => self.focus_lost(),
                WindowEvent::Focused(true) => self.release(),
                WindowEvent::Occluded(hidden) => self.hidden = hidden,
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
    }

    #[cfg(test)]
    fn key(&mut self, key: KeyCode, state: ElementState, blocked: bool, ctx: &egui::Context) {
        self.keyboard(
            HostKey {
                key,
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
            presenter.key_event(key, state);
        }
    }

    fn consume_hotkey(&mut self, key: KeyCode) {
        self.consumed.insert(key);
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
        }
        if self.consumed.contains(&key) {
            if !pressed {
                self.consumed.remove(&key);
            }
            return;
        }
        if !blocked && fresh {
            if let Some(command) = shell_model::hotkey(
                key,
                self.modifiers[..2].contains(&true),
                self.modifiers[2..4].contains(&true),
                self.modifiers[4..].contains(&true),
                self.state(),
                self.guest_focus,
            ) {
                self.consume_hotkey(key);
                self.command(command, ctx);
                return;
            }
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
        let has_dialog = self.has_dialog();
        let Some(presenter) = &mut self.presenter else {
            return;
        };
        if presenter.running {
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
                self.message = "Machine halted — Stop returns to the library".into();
                self.paused = true;
            }
        }
        if let Some(deadline) = deadline {
            ctx.request_repaint_after(deadline.saturating_sub(elapsed));
        }
        if presenter.audio.check_failure(&mut io::stderr()) {
            self.message = "Host audio unavailable".into();
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_readiness(ctx);
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
                gradient_title(ui, &self.title());
            }
            self.menus(ui, &mut commands);
            if self.toolbar {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    for (label, icon, command) in [
                        ("Start", crate::shell_icons::Icon::Play, Command::Start),
                        (
                            if self.paused { "Resume" } else { "Pause" },
                            if self.paused {
                                crate::shell_icons::Icon::Play
                            } else {
                                crate::shell_icons::Icon::Pause
                            },
                            Command::Pause,
                        ),
                        ("Stop", crate::shell_icons::Icon::Stop, Command::Stop),
                        (
                            "Ctrl+Alt+Del",
                            crate::shell_icons::Icon::Reset,
                            Command::Reset,
                        ),
                        ("A:", crate::shell_icons::Icon::Floppy, Command::Insert(0)),
                        ("Eject", crate::shell_icons::Icon::Eject, Command::Eject(0)),
                        (
                            "Settings",
                            crate::shell_icons::Icon::Settings,
                            Command::Placeholder("Settings"),
                        ),
                        (
                            "Layers",
                            crate::shell_icons::Icon::Layers,
                            Command::Placeholder("Disk layers"),
                        ),
                    ] {
                        let response = ui
                            .add_enabled_ui(self.enablement().allows(command), |ui| {
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
                });
            }
        });
        if self.statusbar {
            egui::Panel::bottom("statusbar").show(ui, |ui| self.status(ui));
        }
        self.screen = egui::Rect::NOTHING;
        if self.presenter.is_some() {
            self.display(ui);
        } else {
            self.welcome(ui, &mut commands);
        }
        let menu_open = egui::Popup::is_any_open(&ctx);
        let had_dialog = self.has_dialog();
        let mut close_dialog = false;
        let mut open_machine = false;
        let mut save_preferences = false;
        if let Some((title, text)) = &self.dialog {
            egui::Modal::new(egui::Id::unique("shell-dialog")).show(&ctx, |ui| {
                ui.set_max_width(600.0);
                if self.classic {
                    gradient_title(ui, title);
                } else {
                    ui.heading(*title);
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
                    if !self.discovery_warnings.is_empty() {
                        ui.separator();
                        ui.label("Skipped firmware search entries:");
                        for warning in &self.discovery_warnings {
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
                } else if ui.button("OK").clicked() {
                    close_dialog = true;
                }
            });
        }
        self.media_modals(&ctx);
        if open_machine {
            self.open_machine();
        }
        if save_preferences {
            self.save_preferences();
        }
        if close_dialog {
            self.dialog = None;
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
        self.tick(&ctx, blocked, self.elapsed());
        self.sync_routing();
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
        if ctx.input(|input| input.viewport().close_requested()) {
            self.stop();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop();
    }
}

fn configure_context(ctx: &egui::Context) {
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
fn bevel(ui: &egui::Ui, rect: egui::Rect, pressed: bool) {
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

fn gradient_title(ui: &mut egui::Ui, title: &str) {
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
    ui.painter().text(
        rect.left_center() + Vec2::new(7.0, 0.0),
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

const SHORTCUTS: &str = "Enter / double-click: start selected machine\nCtrl+Alt+P: pause / resume\nCtrl+Alt+Q: stop and return to library\nCtrl+Alt+End: send guest Ctrl+Alt+Del\nCtrl+Alt+F: next A: image (Shift for B:)\nCtrl+Alt+E: eject A: (Shift for B:)\nCtrl+Alt+I: insert A: floppy (Shift for B:)\nCtrl+Alt+M: mute host audio\nCtrl+Alt+G: release pointer\nCtrl+Alt+Enter: full screen\nCtrl+N: new machine (not yet available)\nCtrl+O: open machine file\nCtrl+,: settings (not yet available)\nF1: keyboard shortcuts (outside guest focus)\nCtrl+N/O/, also require shell focus while running.\n\nClick the emulated screen to focus guest input and capture the pointer.\nMenus, dialogs and focus loss release the pointer and held guest keys.";

#[cfg(test)]
mod tests {
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
            events: Rc::default(),
            errors: Rc::default(),
            window: None,
            screen: egui::Rect::NOTHING,
            guest_focus: true,
            pointer: egui::Pos2::ZERO,
            modifiers: [false; 6],
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
            start_request: None,
            checked_selection: None,
            insert_dialog: None,
            blank_disk: None,
            discovery_warnings: BTreeSet::new(),
        }
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
            shell.command(Command::Insert(0), &ctx);
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
        shell.command(Command::Insert(0), &ctx);
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
        shell.command(Command::Insert(0), &ctx);
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
        shell.command(Command::Insert(0), &ctx);
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
            shell.command(Command::Insert(0), &ctx);
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
                .keyboard(key, Pressed, true, false);
            shell.started(presenter);
            assert_eq!(shell.modifiers, [false; 6]);
            assert!(shell.held.is_empty());
            assert!(shell.consumed.is_empty());
            shell
                .events
                .borrow_mut()
                .keyboard(key, Pressed, true, false);
            shell.events.borrow_mut().key(key, Released);
            shell.input(&ctx, false);
            assert!(take_guest_scancodes(&mut shell).is_empty(), "{key:?}");
            assert!(shell.guest_held.is_empty());
            shell.stop();
            assert_eq!(shell.modifiers, [false; 6]);
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
        assert_eq!(shell.modifiers, [false; 6]);
        shell
            .events
            .borrow_mut()
            .keyboard(KeyCode::ControlLeft, Pressed, false, true);
        shell
            .events
            .borrow_mut()
            .keyboard(KeyCode::ControlLeft, Pressed, true, false);
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
        assert_eq!(
            presenter.dropped_clocks,
            crate::pacing::virtual_ticks(Duration::from_millis(200))
        );
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
        assert_eq!(shell.modifiers, [false; 6]);
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
            .advance(std::time::Duration::from_millis(20))
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
    fn stop_skips_the_already_finished_halt_session() {
        let temp = assembly::test_support::TempDir::new("shell-finished-halt");
        let mut shell = journal_shell(&temp, vec![0xfa, 0xf4]);
        shell
            .presenter
            .as_mut()
            .unwrap()
            .advance(std::time::Duration::from_millis(20))
            .unwrap();
        assert!(!shell.presenter.as_ref().unwrap().running);
        assert!(shell.presenter.as_ref().unwrap().error.is_none());
        let size = std::fs::metadata(temp.path().join("disk.upfj"))
            .unwrap()
            .len();
        assert_eq!(size, 128 + 640);
        std::fs::OpenOptions::new()
            .write(true)
            .open(temp.path().join("base.img"))
            .unwrap()
            .set_len(512)
            .unwrap();
        assert!(shell.stop());
        assert!(shell.errors.borrow().is_none());
        assert_eq!(
            std::fs::metadata(temp.path().join("disk.upfj"))
                .unwrap()
                .len(),
            size
        );
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
        for option in ["--help", "--about", "--version"] {
            let output = crate::informational_output(std::ffi::OsStr::new(option)).unwrap();
            assert!(output.contains(window::EGUI_FONT_NOTICE));
        }
        for credit in [
            "Ubuntu-font-1.0",
            "OFL-1.1",
            "Source Foundry",
            "Canonical",
            "John Slegers",
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
        let mut output = ctx.run_pass(Default::default(), |ctx| shell.command(Command::Exit, ctx));
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
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        shell.refresh_library();
        shell.apply_readiness(job.index, job.revision, prepared);
        assert!(!shell.library[0].checked);
        assert!(!shell.library[1].checked);
        shell.poll_readiness(&ctx);
        let job = shell.readiness.take().unwrap();
        let prepared = job
            .receiver
            .recv_timeout(std::time::Duration::from_secs(10))
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
            .recv_timeout(std::time::Duration::from_secs(10))
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
}
